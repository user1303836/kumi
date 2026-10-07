"""Loadable Ableton Control Surface entrypoint for Ableton MCP.

The package keeps configuration outside the Remote Script.  Live invokes
    ``create_instance(c_instance)`` with one argument. Configuration is loaded
    from a fixed adjacent non-secret reference to a separate owner-controlled
    file, so Live does not need command-line or ambient environment secrets.
"""

from __future__ import annotations

import base64
import json
import os
import stat
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any

try:  # Live supplies this module; local contract tests deliberately do not.
    from _Framework.ControlSurface import ControlSurface as _ControlSurface
except ImportError:  # pragma: no cover - exercised only outside Live
    class _ControlSurface:
        def __init__(self, c_instance: Any) -> None:
            self._c_instance = c_instance

try:
    from .ableton_mcp_remote_script import AbletonMcpBridge as _Bridge
    from .ableton_mcp_remote_script import _DIAGNOSTICS_ACCEPTED_MAX_BYTES
except ImportError:  # source-tree contract tests import the flat module
    from ableton_mcp_remote_script import AbletonMcpBridge as _Bridge
    from ableton_mcp_remote_script import _DIAGNOSTICS_ACCEPTED_MAX_BYTES


def _normalize_bridge_config(value: Any) -> dict[str, Any]:
    """Collapse a supported host or bridge configuration to its version-1 shape."""
    if not isinstance(value, dict):
        raise ValueError("unsupported bridge configuration")
    if value.get("version") == 2:
        server = value.get("server")
        bridge = value.get("bridge")
        if (set(value) != {"version", "server", "bridge"} or not isinstance(server, dict) or set(server) != {"command", "args"}
                or not isinstance(server.get("command"), str) or not server["command"] or not isinstance(server.get("args"), list) or any(not isinstance(item, str) for item in server["args"])
                or not isinstance(bridge, dict) or not {"host", "port", "secretFile", "timeoutMs"} <= set(bridge) or set(bridge) - {"host", "port", "secretFile", "timeoutMs", "realtimePort", "diagnostics"}):
            raise ValueError("unsupported bridge configuration")
        timeout_ms = bridge["timeoutMs"]
        if not isinstance(timeout_ms, int) or isinstance(timeout_ms, bool) or not 100 <= timeout_ms <= 60000:
            raise ValueError("unsupported bridge configuration")
        realtime_port = bridge.get("realtimePort")
        if realtime_port is not None and (not isinstance(realtime_port, int) or isinstance(realtime_port, bool) or not 1 <= realtime_port <= 65535 or realtime_port == bridge.get("port")):
            raise ValueError("unsupported bridge configuration")
        diagnostics = bridge.get("diagnostics")
        max_bytes = diagnostics.get("maxBytes") if isinstance(diagnostics, dict) else None
        if diagnostics is not None and (not isinstance(diagnostics, dict) or set(diagnostics) != {"path", "maxBytes"} or not isinstance(diagnostics.get("path"), str) or not Path(diagnostics["path"]).is_absolute() or not isinstance(max_bytes, int) or isinstance(max_bytes, bool) or max_bytes not in _DIAGNOSTICS_ACCEPTED_MAX_BYTES):
            raise ValueError("unsupported bridge diagnostics configuration")
        normalized = {"version": 1, "host": bridge["host"], "port": bridge["port"], "secretFile": bridge["secretFile"]}
        if realtime_port is not None:
            normalized["realtimePort"] = realtime_port
        if diagnostics is not None:
            normalized["diagnostics"] = diagnostics
        return normalized
    if set(value) != {"version", "host", "port", "secretFile"} or value.get("version") != 1:
        raise ValueError("unsupported bridge configuration")
    return value


def _mode_owner_only(path: Path) -> bool:
    """Apply POSIX mode-bit checks only where those bits carry POSIX meaning."""
    if os.name == "nt":
        return True
    try:
        return stat.S_IMODE(path.stat().st_mode) & 0o077 == 0
    except OSError:
        return False


def _local_path(path: Path) -> bool:
    r"""Whether a path is an absolute one on this machine: not a share's (two leading separators in any mix,
    \\host\share or //host/share) nor one in a device namespace (\\?\, \\.\ or \??\), where even a stat can reach
    another host."""
    return path.is_absolute() and not str(path).replace("/", "\\").startswith(("\\\\", "\\??\\"))


def _config_paths(reference: Path) -> list[Path]:
    """The files _read_config checks the owner of, as far as they can be read now: its checks then find their
    verdicts waiting, one PowerShell run in all on Windows instead of one each. Nothing in them is trusted until those
    checks pass, so no symlink is followed (they refuse one anyway) and a path that isn't a _local_path isn't touched
    at all, left to its own check: a reference or config another account can write mustn't have Live contact a host
    it names."""
    looked_at = lambda path: _local_path(path) and not path.is_symlink()
    paths: list[Path] = []
    try:
        if not looked_at(reference): return paths
        paths.append(reference)
        config = Path(json.loads(reference.read_text(encoding="utf-8"))["config"])
        if not looked_at(config): return paths
        paths.append(config)
        value = _normalize_bridge_config(json.loads(config.read_text(encoding="utf-8")))
        diagnostics = Path(value["diagnostics"]["path"]) if isinstance(value.get("diagnostics"), dict) else None
        named = [Path(value["secretFile"])] + ([diagnostics, diagnostics.parent] if diagnostics is not None else [])
        paths += [path for path in named if looked_at(path)]
    except (OSError, UnicodeError, ValueError, KeyError, TypeError):
        pass
    return paths


def _read_config() -> dict[str, Any]:
    reference = Path(__file__).with_name("bridge-reference.json")
    if os.name == "nt":
        _prefetch_acl_verdicts(_config_paths(reference))
    if reference.is_symlink() or not reference.is_file() or not _owner_controlled(reference) or not _mode_owner_only(reference):
        raise ValueError("bridge configuration reference is missing or unsafe")
    try:
        reference_value = json.loads(reference.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("bridge configuration reference is unreadable or malformed") from error
    if not isinstance(reference_value, dict) or set(reference_value) != {"config"} or not isinstance(reference_value["config"], str):
        raise ValueError("bridge configuration reference is invalid")
    path = Path(reference_value["config"])
    if not path.is_absolute() or path.is_symlink() or not path.is_file() or not _owner_controlled(path):
        raise ValueError("bridge configuration must be an existing regular file")
    if not _mode_owner_only(path):
        raise ValueError("bridge configuration must be owner-readable")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("bridge configuration is unreadable or malformed") from error
    value = _normalize_bridge_config(value)
    host, port, secret_file = value.get("host"), value.get("port"), value.get("secretFile")
    if host not in {"127.0.0.1", "::1"} or not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535 or not isinstance(secret_file, str):
        raise ValueError("bridge configuration is invalid")
    realtime_port = value.get("realtimePort")
    if realtime_port is not None and (not isinstance(realtime_port, int) or isinstance(realtime_port, bool) or not 1 <= realtime_port <= 65535 or realtime_port == port):
        raise ValueError("realtime port is invalid")
    secret_path = Path(secret_file)
    if not secret_path.is_absolute() or secret_path.is_symlink() or not secret_path.is_file() or not _owner_controlled(secret_path):
        raise ValueError("bridge secret must be an existing regular file")
    if not _mode_owner_only(secret_path):
        raise ValueError("bridge secret must be owner-readable")
    secret = secret_path.read_text(encoding="utf-8")
    if secret.endswith("\n"):
        secret = secret[:-1]
    if not secret or len(secret) < 32 or any(character.isspace() for character in secret):
        raise ValueError("bridge secret is invalid")
    result = {"host": host, "port": port, "secret": secret, "realtimePort": realtime_port}
    diagnostics = value.get("diagnostics")
    if isinstance(diagnostics, dict) and _diagnostics_path_safe(Path(diagnostics["path"])):
        result["diagnostics"] = diagnostics
    return result


def _owner_controlled(path: Path) -> bool:
    """Require the current account to own each security-sensitive file."""
    if os.name == "nt":
        # The ACL check compares the owner with the process token too, so it decides alone where the
        # native comparison can't be made: Live's own Python on Windows comes without ctypes.
        return _windows_owner_controlled(path) is not False and _windows_acl_owner_only(path)
    try:
        return path.stat().st_uid == os.getuid()
    except (AttributeError, OSError):
        return False


def _diagnostics_path_safe(path: Path) -> bool:
    """Validate the provisioned diagnostics leaf and its private parent."""
    reparse = lambda entry: bool(getattr(entry, "st_file_attributes", 0) & 0x400)
    try:
        if not path.is_absolute() or path.is_symlink():
            return False
        leaf = path.lstat()
        if reparse(leaf) or not stat.S_ISREG(leaf.st_mode) or leaf.st_nlink != 1 or not _owner_controlled(path) or not _mode_owner_only(path):
            return False
        parent = path.parent
        parent_entry = parent.lstat()
        if reparse(parent_entry) or not stat.S_ISDIR(parent_entry.st_mode) or parent.is_symlink() or not _owner_controlled(parent) or not _mode_owner_only(parent):
            return False
        for ancestor in (parent, *parent.parents):
            entry = ancestor.lstat()
            if (stat.S_ISLNK(entry.st_mode) or reparse(entry)) and str(ancestor) not in {"/var", "/tmp"}:
                return False
        return True
    except (AttributeError, OSError, ValueError):
        return False


# Verdicts of the Windows ACL check, by the path and what's on disk there (device, file id, size, times), each for a
# short while: the Control Surface's start asks about the same few files more than once, and each check of Live's
# own Python (it has no ctypes) is a PowerShell run on Live's main thread.
_ACL_VERDICTS: dict[tuple[Any, ...], tuple[bool, float]] = {}
_ACL_VERDICT_SECONDS = 30.0
# A batch's answer: this bit set, and bit i for path i that failed. Any other exit (PowerShell failing to start, a
# script error's 1) fails every path, closed.
_ACL_ANSWERED = 128
_ACL_BATCH = 7


def _acl_key(path: Path) -> tuple[Any, ...] | None:
    try:
        entry = os.stat(path)
    except (OSError, ValueError, TypeError):
        return None
    return (str(path), entry.st_dev, entry.st_ino, entry.st_size, entry.st_mtime_ns, getattr(entry, "st_ctime_ns", None))


def _keep_acl_verdict(key: tuple[Any, ...], verdict: bool) -> None:
    """Keep a verdict, dropping those past their time: a file that grows (the diagnostics log, checked once a minute
    for as long as Live runs) has a new key at each check. The diagnostics writer's thread keeps verdicts too, hence
    the copy and the pop."""
    now = time.monotonic()
    for held, (_, at) in list(_ACL_VERDICTS.items()):
        if now - at >= _ACL_VERDICT_SECONDS: _ACL_VERDICTS.pop(held, None)
    _ACL_VERDICTS[key] = (verdict, now)


def _acl_cached(key: tuple[Any, ...] | None) -> bool | None:
    held = _ACL_VERDICTS.get(key) if key is not None else None
    return held[0] if held is not None and time.monotonic() - held[1] < _ACL_VERDICT_SECONDS else None


def _windows_acl_owner_only(path: Path) -> bool:
    """Require a protected DACL containing exactly one owner FullControl ACE.

    Verification uses the Windows security API with explicit exit codes so no
    localized or serialized output is parsed.
    """
    key = _acl_key(path); cached = _acl_cached(key)
    if cached is not None:
        return cached
    verdict = _windows_acl_owner_only_all([path])[0]
    if key is not None:
        _keep_acl_verdict(key, verdict)
    return verdict


def _prefetch_acl_verdicts(paths: list[Path]) -> None:
    """Check several files' ACLs in one PowerShell run and keep the verdicts, so the checks after it don't each
    start one. Best effort: a path that can't be looked at is left for its own check."""
    wanted = []
    for path in paths:
        key = _acl_key(path) if path is not None else None
        if key is not None and _acl_cached(key) is None and key not in [held for _, held in wanted]:
            wanted.append((path, key))
    for start in range(0, len(wanted), _ACL_BATCH):
        batch = wanted[start:start + _ACL_BATCH]
        verdicts = _windows_acl_owner_only_all([path for path, _ in batch])
        for (_, key), verdict in zip(batch, verdicts):
            _keep_acl_verdict(key, verdict)


def _windows_acl_owner_only_all(paths: list[Path]) -> list[bool]:
    """The ACL check for up to _ACL_BATCH paths in one PowerShell run, by its exit code (no output is parsed)."""
    if not 1 <= len(paths) <= _ACL_BATCH:
        return [False] * len(paths)
    try:
        encoded = base64.b64encode("\n".join(str(path) for path in paths).encode("utf-8")).decode("ascii")
        script = (
            "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;"
            "function OwnerOnly($p) {"
            "$c=[System.IO.File]::GetAccessControl($p);"
            "if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { return $false }"
            "if (-not $c.AreAccessRulesProtected) { return $false }"
            "$rules=@($c.Access); if ($rules.Count -ne 1) { return $false }"
            "$rule=$rules[0];"
            "if ($rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { return $false }"
            "if ($rule.IsInherited) { return $false }"
            "if ($rule.AccessControlType.ToString() -ne 'Allow') { return $false }"
            "if (($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { return $false }"
            "return $true };"
            "$paths=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATHS)) -split \"`n\";"
            f"$mask={_ACL_ANSWERED}; $i=0;"
            "foreach ($p in $paths) { $ok=$false; try { $ok=(OwnerOnly $p) -eq $true } catch { $ok=$false }; if (-not $ok) { $mask=$mask -bor (1 -shl $i) }; $i++ };"
            "exit $mask"
        )
        environment = dict(os.environ)
        environment["ABLETON_MCP_ACL_PATHS"] = encoded
        # By its full path: a bare name is looked for in Live's own folder and the working folder first.
        powershell = os.path.join(os.environ.get("SYSTEMROOT") or r"C:\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
        # Live has no console of its own: without this flag each check would open a console window.
        result = subprocess.run(
            [powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script],
            capture_output=True, timeout=10, env=environment, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        code = result.returncode
        if not isinstance(code, int) or code & ~((1 << len(paths)) - 1) != _ACL_ANSWERED:
            return [False] * len(paths)
        return [not code & (1 << index) for index in range(len(paths))]
    except (AttributeError, OSError, ValueError, TypeError, subprocess.SubprocessError):
        return [False] * len(paths)


def _windows_owner_controlled(path: Path) -> bool | None:
    """Compare the file owner SID with the current process token on Windows.

    ``stat().st_uid`` is not a Windows security identity and is commonly zero
    or otherwise synthetic on Windows.  Use the native security descriptor and
    token APIs instead, without adding a platform-specific dependency.  None
    where ctypes isn't there to ask with (Live's own Python on Windows).
    """
    security_descriptor = None
    try:
        try:
            import ctypes
            from ctypes import wintypes
        except ImportError:
            return None

        advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

        owner_sid = ctypes.c_void_p()
        security_descriptor = ctypes.c_void_p()
        advapi32.GetNamedSecurityInfoW.argtypes = [
            wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
            ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p,
            ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
        ]
        advapi32.GetNamedSecurityInfoW.restype = wintypes.DWORD
        if advapi32.GetNamedSecurityInfoW(
            str(path), 1, 1, ctypes.byref(owner_sid), None, None, None,
            ctypes.byref(security_descriptor),
        ) != 0 or not owner_sid.value:
            return False

        token = wintypes.HANDLE()
        kernel32.GetCurrentProcess.argtypes = []
        kernel32.GetCurrentProcess.restype = wintypes.HANDLE
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        kernel32.LocalFree.argtypes = [ctypes.c_void_p]
        kernel32.LocalFree.restype = ctypes.c_void_p
        advapi32.OpenProcessToken.argtypes = [
            wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE),
        ]
        advapi32.OpenProcessToken.restype = wintypes.BOOL
        if not advapi32.OpenProcessToken(kernel32.GetCurrentProcess(), 0x0008, ctypes.byref(token)):
            return False
        try:
            class SidAndAttributes(ctypes.Structure):
                _fields_ = [("sid", ctypes.c_void_p), ("attributes", wintypes.DWORD)]

            class TokenUser(ctypes.Structure):
                _fields_ = [("user", SidAndAttributes)]

            class TokenOwner(ctypes.Structure):
                _fields_ = [("owner", ctypes.c_void_p)]

            advapi32.GetTokenInformation.argtypes = [
                wintypes.HANDLE, wintypes.DWORD, ctypes.c_void_p,
                wintypes.DWORD, ctypes.POINTER(wintypes.DWORD),
            ]
            advapi32.GetTokenInformation.restype = wintypes.BOOL

            def token_information(info_class: int, structure: Any) -> tuple[Any, Any] | None:
                required = wintypes.DWORD()
                advapi32.GetTokenInformation(token, info_class, None, 0, ctypes.byref(required))
                if not required.value:
                    return None
                buffer = ctypes.create_string_buffer(required.value)
                if not advapi32.GetTokenInformation(
                    token, info_class, buffer, required.value, ctypes.byref(required),
                ):
                    return None
                return buffer, ctypes.cast(buffer, ctypes.POINTER(structure)).contents

            user_info = token_information(1, TokenUser)
            owner_info = token_information(4, TokenOwner)
            if user_info is None or owner_info is None:
                return False
            # Keep both backing buffers alive while comparing their SID pointers.
            user_buffer, token_user = user_info
            owner_buffer, token_owner = owner_info
            current_sid = token_user.user.sid
            default_owner_sid = token_owner.owner
            advapi32.EqualSid.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
            advapi32.EqualSid.restype = wintypes.BOOL
            return bool(
                current_sid
                and default_owner_sid
                and (
                    advapi32.EqualSid(owner_sid, current_sid)
                    or advapi32.EqualSid(owner_sid, default_owner_sid)
                )
            )
        finally:
            kernel32.CloseHandle(token)
    except (AttributeError, OSError, TypeError, ValueError):
        return False
    finally:
        try:
            if 'kernel32' in locals() and security_descriptor is not None and security_descriptor.value:
                kernel32.LocalFree(security_descriptor)
        except (AttributeError, OSError):
            pass


class _WillingtonProvider:
    """Optional owner of separately installed, exact-build native adapters.

    No ambient environment flags or loading paths supplied by MCP clients.
    The adjacent owner-only file is local operator configuration, not payload.
    """
    def __init__(self, mapper: Any, log: Any):
        self.mapper = mapper
        self.follow = None
        self.devices = None
        self.zones = None
        self.live = None
        self.mapper.willington_follow_writes = False
        self.mapper.willington_device_writes = False
        self.mapper.willington_zone_writes = False
        path = Path(__file__).with_name("willington.json")
        if not path.exists(): return
        try:
            if path.is_symlink() or not path.is_file() or not _owner_controlled(path) or not _mode_owner_only(path) or path.stat().st_size > 4096:
                raise ValueError("unsafe extension configuration")
            config = json.loads(path.read_text())
            required = {"version", "followActions", "deviceTools", "enableWrites"}
            if not isinstance(config, dict) or not required <= set(config) <= required | {"rackZones"} or type(config["version"]) is not int or config["version"] != 1 or any(type(config[key]) is not bool for key in ("followActions", "deviceTools", "enableWrites")) or type(config.get("rackZones", False)) is not bool:
                raise ValueError("invalid extension configuration")
            import Live
            if getattr(Live, "_kumi_willington_owner", None) is not None:
                raise ValueError("native extension already owned")
            if getattr(Live, "_willington_native_library", None) is not None and not getattr(Live, "_kumi_willington_registered", False):
                raise ValueError("standalone Follow Action surface already installed; restart required")
            if any(any(getattr(cls, name, None) is method for cls, name, method in getattr(item, "patches", ())) for item in getattr(Live, "_willington_device_libraries", ())):
                raise ValueError("standalone device surface already installed")
            chain_class = getattr(getattr(Live, "Chain", None), "Chain", None)
            if callable(getattr(chain_class, "get_zone", None)) or callable(getattr(chain_class, "set_zone", None)):
                raise ValueError("rack zone bindings already installed")
            self.live = Live
            Live._kumi_willington_owner = self
            # Kumi's own copy of Willington, inside this package where Live lists no Control Surfaces, for
            # when none is installed beside the package: providers put there by hand come first.
            bundled = Path(__file__).with_name("willington")
            if bundled.is_dir() and str(bundled) not in sys.path:
                sys.path.append(str(bundled))
            try:
                from WillingtonRuntime import ComponentUnavailableError as unavailable
            except ImportError:
                unavailable = ()  # Older packages have no typed availability error.
            try:
                import WillingtonRuntime as willington_runtime
            except ImportError:
                willington_runtime = None
            if willington_runtime is not None:
                _keep_willington_identity(willington_runtime)

            def install_component(component, module):
                if component in getattr(Live, "_kumi_willington_unavailable_components", ()):
                    return None
                from importlib import import_module
                try:
                    return import_module(module).install()
                except unavailable as error:
                    # A no-profile refusal precedes native loading/patching and cannot
                    # change in this Live process. Integrity/installation errors retry.
                    refused = set(getattr(Live, "_kumi_willington_unavailable_components", ()))
                    refused.add(component)
                    Live._kumi_willington_unavailable_components = refused
                    if callable(log): log(str(error))
                    return None

            if config["followActions"]:
                # Clip properties retain native function pointers for this Live process.
                # The Follow package has no uninstall; reuse our disabled registration
                # on reconnect instead of installing those properties a second time.
                self.follow = getattr(Live, "_kumi_willington_follow_library", None)
                if self.follow is None:
                    self.follow = install_component("WillingtonBindings", "WillingtonBindings")
                    if self.follow is not None:
                        Live._kumi_willington_follow_library = self.follow
                if self.follow is not None:
                    self.follow.willington_enable_writes(False)
                    Live._kumi_willington_registered = True
            if config["deviceTools"]:
                self.devices = install_component("WillingtonDeviceTools", "WillingtonDeviceTools.api")
            if config.get("rackZones", False):
                self.zones = install_component("WillingtonRackZones", "WillingtonRackZones.api")
            if config["enableWrites"]:
                # Follow bindings require evidence for this exact compiled library,
                # matching the standalone adapter's operator enablement contract.
                if self.follow is not None:
                    try:
                        import hashlib
                        import WillingtonBindings
                        folder = Path(WillingtonBindings.__file__).parent
                        evidence = json.loads((folder / "self-test.json").read_text())
                        library_path = Path(getattr(self.follow, "path", folder / "libwillington.dylib"))
                        digest = hashlib.sha256(library_path.read_bytes()).hexdigest()
                        if evidence.get("status") != "passed" or evidence.get("library_sha256") != digest:
                            raise ValueError("current-library Follow Action self-test is required")
                        self.follow.willington_enable_writes(True)
                        self.mapper.willington_follow_writes = True
                    except Exception as error:
                        if callable(log): log("Willington Follow Action writes unavailable: " + str(error))
                if self.devices is not None:
                    self.devices.enable(True)
                    self.mapper.willington_device_writes = True
                if self.zones is not None:
                    self.zones.enable(True)
                    self.mapper.willington_zone_writes = True
            if callable(log):
                components = (("Follow Actions", self.follow, self.mapper.willington_follow_writes),
                              ("Device Tools", self.devices, self.mapper.willington_device_writes),
                              ("Rack Zones", self.zones, self.mapper.willington_zone_writes))
                active = [name for name, provider, _ in components if provider is not None]
                writable = [name for name, _, enabled in components if enabled]
                log("Willington extensions initialized; active providers: " + (", ".join(active) or "none")
                    + "; writes enabled: " + (", ".join(writable) or "none"))
        except Exception as error:
            try: self.close()
            except Exception: pass  # Capability flags are cleared even if native teardown fails.
            if callable(log): log("Willington extensions unavailable: " + str(error) + "; ordinary bridge remains active")

    def close(self):
        self.mapper.willington_follow_writes = False
        self.mapper.willington_device_writes = False
        self.mapper.willington_zone_writes = False
        try:
            if self.follow is not None: self.follow.willington_enable_writes(False)
        finally:
            try:
                if self.devices is not None: self.devices.uninstall()
            finally:
                try:
                    if self.zones is not None: self.zones.uninstall()
                finally:
                    if self.live is not None and getattr(self.live, "_kumi_willington_owner", None) is self:
                        self.live._kumi_willington_owner = None
                    self.follow = self.devices = self.zones = None


_WILLINGTON_CHECK_SECONDS = 1.0


def _keep_willington_identity(runtime: Any) -> None:
    """Willington's identity() hashes the whole Live executable (SHA-256) each call, and its components' installs
    ask for it up to five times a provider, on every /willington switch too, on Live's main thread. What it reads (the
    running Live's version and executable) can't change while Live runs, so its answer is kept for the life of the
    process. vendor/willington keeps Willington's own files as they ship; the wrapper is Kumi's."""
    original = getattr(runtime, "identity", None)
    if not callable(original) or getattr(original, "_kumi_kept", False):
        return
    kept: list[dict[str, Any]] = []

    def identity() -> dict[str, Any]:
        if not kept:
            kept.append(original())
        return dict(kept[0])

    identity._kumi_kept = True  # type: ignore[attr-defined]
    runtime.identity = identity


def _willington_switch() -> tuple[int, int, int] | None:
    """The Willington opt-in file as it stands, None when there's none: what tells that it changed."""
    try:
        entry = Path(__file__).with_name("willington.json").stat()
    except OSError:
        return None
    return entry.st_mtime_ns, entry.st_size, entry.st_ino


class AbletonMcpBridge(_ControlSurface):
    """Control Surface lifecycle wrapper around the dependency-free bridge."""

    def __init__(self, c_instance: Any) -> None:
        super().__init__(c_instance)
        accessor = getattr(self, "song", None)
        self._bridge = _Bridge(c_instance, _read_config(), song=accessor() if callable(accessor) else None, provenance="real-live", diagnostics_validator=_diagnostics_path_safe)
        self._disconnected = False
        self._willington = None
        self._timer = None
        self._schedule_next()
        self._start_timer()

    def _start_timer(self) -> None:
        """Live's own timer serves the bridge between display ticks, where this Live has one; without it,
        each display tick serves it, as before."""
        try:
            import Live  # type: ignore[import-not-found]
            timer = Live.Base.Timer(callback=self._between_ticks, interval=1, repeat=True)
            timer.start()
        except Exception:
            return
        self._timer = timer
        self._timer_calls, self._timer_counted_from = 0, time.perf_counter()
        self._bridge.between_ticks = True

    def _stop_timer(self) -> None:
        timer, self._timer = getattr(self, "_timer", None), None
        bridge = getattr(self, "_bridge", None)
        if bridge is not None: bridge.between_ticks = False
        if timer is not None:
            try: timer.stop()
            except Exception: pass

    def _between_ticks(self) -> None:
        # A stopped timer's callback already on its way does nothing.
        if self._disconnected or self._timer is None:
            return
        counted_from = getattr(self, "_timer_counted_from", None)
        if counted_from is not None:
            # How often this Live calls back, said once: about every 10 ms on a Mac; Windows may differ.
            self._timer_calls += 1
            elapsed = time.perf_counter() - counted_from
            if elapsed >= 5.0:
                self._timer_counted_from = None
                log = getattr(self, "log_message", None)
                if callable(log): log("Bridge timer: " + str(round(self._timer_calls / elapsed)) + " calls a second")
        try:
            self._bridge.serve_between_ticks()
        except Exception as error:
            # The ticks still serve the bridge: a timer that fails once stops, rather than failing a thousand times a second.
            self._stop_timer()
            log = getattr(self, "log_message", None)
            if callable(log): log("Bridge timer stopped (" + type(error).__name__ + ": " + str(error) + "); display ticks serve the bridge\n" + traceback.format_exc(limit=3))

    def _schedule_next(self) -> None:
        scheduler = getattr(self, "schedule_message", None)
        self._scheduled = scheduler(1, self._drain) if not self._disconnected and callable(scheduler) else None

    def _keep_willington(self) -> None:
        """Make the Willington provider, and make it again when its opt-in file changes (Kumi's /willington
        writes or removes it), looking at most once a second: the switch takes effect without Live restarting."""
        now = time.monotonic()
        current = getattr(self, "_willington", None)
        if current is not None and now - getattr(self, "_willington_checked", 0.0) < _WILLINGTON_CHECK_SECONDS:
            return
        self._willington_checked = now
        switch = _willington_switch()
        if current is not None and switch == getattr(self, "_willington_seen", None):
            return
        log = getattr(self, "log_message", None)
        if current is not None:
            try:
                current.close()
            except Exception:
                if callable(log): log("Willington teardown failed; its changed configuration loads anyway")
            if switch is None and callable(log):
                log("Willington extensions off: willington.json was removed")
        self._willington_seen = switch
        self._willington = _WillingtonProvider(self._bridge.mapper, log)

    def _drain(self) -> None:
        if self._disconnected:
            return
        try:
            self._keep_willington()
            self._bridge.update_display()
        finally:
            # A tick that fails once doesn't end the drain: the next one is scheduled whatever happened.
            self._schedule_next()

    def update_display(self) -> None:
        self._keep_willington()
        self._bridge.update_display()

    def disconnect(self) -> None:
        self._disconnected = True
        self._stop_timer()
        if self._scheduled is not None:
            self._scheduled = None
        if self._willington is not None:
            try: self._willington.close()
            except Exception:
                log = getattr(self, "log_message", None)
                if callable(log): log("Willington teardown failed; bridge disconnect continues")
        self._bridge.disconnect()
        parent_disconnect = getattr(super(), "disconnect", None)
        if callable(parent_disconnect):
            parent_disconnect()

    @property
    def address(self) -> Any:
        return self._bridge.address


def create_instance(c_instance: Any) -> AbletonMcpBridge:
    return AbletonMcpBridge(c_instance)


__all__ = ["AbletonMcpBridge", "create_instance"]
