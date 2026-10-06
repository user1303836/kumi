"""Exact-build selection shared by Remote Script providers (no native imports)."""
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import sys

LIBRARIES = {
    'WillingtonBindings': 'libwillington.dylib',
    'WillingtonDeviceTools': 'libwillington_devices.dylib',
    'WillingtonRackZones': 'libwillington_zones.dylib',
}
# Live on Windows embeds Python without ctypes; adapters there are extension modules.
WINDOWS_LIBRARIES = {
    'WillingtonBindings': 'willington_bindings.pyd',
    'WillingtonDeviceTools': 'willington_devices.pyd',
    'WillingtonRackZones': 'willington_zones.pyd',
}


def library_name(component, platform_name):
    return (WINDOWS_LIBRARIES if platform_name == 'windows' else LIBRARIES)[component]


class ComponentUnavailableError(RuntimeError):
    """No validated profile exists; raised before loading or modifying bindings."""


def digest(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def process_executable():
    # Embedded Python's sys.executable is not guaranteed to identify Live.
    # Ask the loader for the actual process image.
    if sys.platform == 'win32':
        import _winapi
        return Path(_winapi.GetModuleFileName(0)).resolve(strict=True)
    if sys.platform != 'darwin':
        return Path(sys.executable).resolve(strict=True)
    try:
        import _ctypes as ct
    except ImportError:
        import ctypes as ct  # Live exposes the raw native module here as well.

    class Pointer(ct._SimpleCData):
        _type_ = 'P'
    class Size(ct._SimpleCData):
        _type_ = 'I'
    class Integer(ct._SimpleCData):
        _type_ = 'i'
    class Byte(ct._SimpleCData):
        _type_ = 'c'
    class GetPath(ct.CFuncPtr):
        _flags_ = ct.FUNCFLAG_CDECL
        _argtypes_ = (Pointer, Pointer)
        _restype_ = Integer

    handle = ct.dlopen(None, os.RTLD_NOW | os.RTLD_LOCAL)
    try:
        get_path = GetPath(ct.dlsym(handle, '_NSGetExecutablePath'))
        size = Size(0)
        get_path(None, ct.addressof(size))
        if not 0 < size.value <= 1024 * 1024:
            raise RuntimeError('Cannot determine running Live executable')
        class Buffer(ct.Array):
            _type_ = Byte
            _length_ = size.value
        buffer = Buffer()
        if get_path(ct.addressof(buffer), ct.addressof(size)) != 0:
            raise RuntimeError('Cannot determine running Live executable')
        return Path(os.fsdecode(buffer.value)).resolve(strict=True)
    finally:
        ct.dlclose(handle)


def identity():
    """Read identity inside the connected Live process, never on Kumi's host."""
    import Live
    app = Live.Application.get_application()
    version = app.get_version_string() if callable(getattr(app, 'get_version_string', None)) else (
        '%s.%s.%s' % (app.get_major_version(), app.get_minor_version(), app.get_bugfix_version()))
    architecture = platform.machine().lower()
    architecture = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(architecture, architecture)
    executable = process_executable()
    return {'platform': {'darwin': 'macos', 'win32': 'windows'}.get(sys.platform, sys.platform),
            'architecture': architecture, 'version': version,
            'executable': str(executable), 'source_sha256': digest(executable)}


def select(matrix, runtime, component):
    if matrix.get('schema') != 1:
        raise RuntimeError('Unsupported Willington matrix schema')
    matches = []
    for row in matrix['profiles']:
        if row['status'] != 'validated' or component not in row['components']:
            continue
        if any(row[key] != runtime[key] for key in ('platform', 'architecture', 'source_sha256')):
            continue
        # Some Live APIs omit beta/build suffixes. The executable hash still
        # identifies the exact build; a version string alone never selects code.
        version = row['version']
        versions = (version, version.split(' ')[0], re.match(r'\d+\.\d+\.\d+', version).group())
        if runtime['version'] in versions:
            matches.append(row)
    if len(matches) != 1:
        error = ComponentUnavailableError if not matches else RuntimeError
        raise error('Willington %s unavailable for %s/%s Live %s: %s exact validated profiles' % (
            component, runtime['platform'], runtime['architecture'], runtime['version'], len(matches)))
    return matches[0]


def verify(library, manifest=None, runtime=None):
    """Verify the running executable and artifact, including explicit candidates."""
    library = Path(library).resolve(strict=True)
    manifest = Path(manifest) if manifest else library.with_name('build.json')
    expected = json.loads(manifest.read_text())
    runtime = identity() if runtime is None else runtime
    if runtime['source_sha256'] != expected['source_sha256']:
        raise RuntimeError('Running Live executable differs from binding manifest')
    if digest(library) != expected['library_sha256']:
        raise RuntimeError('Native library differs from binding manifest')
    return expected


def resolve(component, folder, runtime=None, matrix=None):
    runtime = identity() if runtime is None else runtime
    if matrix is None:
        matrix = json.loads(Path(__file__).with_name('matrix.json').read_text())
    row = select(matrix, runtime, component)
    folder = Path(folder)
    name = library_name(component, runtime['platform'])
    # Prefer the matrix layout; retain support for a verified single-build package.
    candidates = (folder / 'build' / row['id'] / name, folder / 'build' / name, folder / name)
    for library in candidates:
        if not library.is_file():
            continue
        manifest = library.with_name('build.json')
        if not manifest.is_file():
            manifest = folder / 'build.json'
        expected = json.loads(manifest.read_text())
        if expected.get('profile_id') != row['id']:
            continue
        if expected.get('validation_status') != 'validated':
            raise RuntimeError('Selected native artifact is not validated')
        verify(library, manifest, runtime)
        return str(library.resolve()), str(manifest.resolve())
    raise RuntimeError('Bindings for %s/%s are not installed' % (row['id'], component))


def load_extension(library):
    """Load a verified Windows adapter module from its exact path (never via sys.path)."""
    import importlib.machinery
    import importlib.util
    library = Path(library).resolve(strict=True)
    name = library.name.split('.')[0]
    loader = importlib.machinery.ExtensionFileLoader(name, str(library))
    spec = importlib.util.spec_from_file_location(name, str(library), loader=loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module
