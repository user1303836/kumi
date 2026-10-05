import base64
import copy
import hashlib
import json
import os
import secrets
import struct
import subprocess
import sys
import types
import tempfile
import threading
import time
import unittest
import select as select_module
from pathlib import Path
from unittest.mock import patch

import ableton_mcp_remote_script as remote_module
from ableton_mcp_remote_script import (
    AbletonMcpBridge,
    AuthenticatedRemoteScript,
    LiveObjectMapper, _DiagnosticsSink, _DispatchToken, _MainThreadQueue, _Subscription, _authority_state_digest, _clear_diagnostics_sink, _debug_trace, _owned_device_row, _set_diagnostics_sink, operation_registry, validate_operation_payload,
    PROTOCOL,
    create_instance,
)
from AbletonMcpBridge import _diagnostics_path_safe, _mode_owner_only, _owner_controlled, _normalize_bridge_config


class BridgeConfigNormalizationTests(unittest.TestCase):
    def test_version_two_host_config_normalizes_to_bridge_shape(self):
        value = {"version": 2, "server": {"command": "node", "args": ["cli.js", "--config", "cfg.json"]}, "bridge": {"host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret", "timeoutMs": 5000, "realtimePort": 9766}}
        self.assertEqual(_normalize_bridge_config(value), {"version": 1, "host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret", "realtimePort": 9766})

    def test_version_two_requires_complete_server_and_bridge_shapes(self):
        with self.assertRaises(ValueError):
            _normalize_bridge_config({"version": 2, "server": {}, "bridge": {"host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret"}})

    def test_realtime_port_must_be_distinct_and_bounded(self):
        value = {"version": 2, "server": {"command": "node", "args": ["cli.js", "--config", "cfg.json"]}, "bridge": {"host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret", "timeoutMs": 5000, "realtimePort": 9765}}
        with self.assertRaises(ValueError):
            _normalize_bridge_config(value)

    def test_version_two_rejects_unknown_timeout_and_extra_keys(self):
        base = {"version": 2, "server": {"command": "node", "args": []}, "bridge": {"host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret", "timeoutMs": 5000}}
        with self.assertRaises(ValueError):
            _normalize_bridge_config({**base, "bridge": {**base["bridge"], "timeoutMs": 30}})
        with self.assertRaises(ValueError):
            _normalize_bridge_config({**base, "bridge": {**base["bridge"], "extra": True}})
        with self.assertRaises(ValueError):
            _normalize_bridge_config({**base, "extra": True})

    def test_version_two_accepts_only_the_bounded_diagnostics_shape(self):
        base = {"version": 2, "server": {"command": "node", "args": []}, "bridge": {"host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/secret", "timeoutMs": 5000}}
        absolute_path = str((Path(tempfile.gettempdir()) / "owner" / "bridge-diagnostics.log").resolve())
        # 16 MiB now; a configuration written before the bound went up (256 KiB) still loads.
        for max_bytes in (16 * 1024 * 1024, 256 * 1024):
            diagnostics = {"path": absolute_path, "maxBytes": max_bytes}
            normalized = _normalize_bridge_config({**base, "bridge": {**base["bridge"], "diagnostics": diagnostics}})
            self.assertEqual(normalized["diagnostics"], diagnostics)
        for invalid in [{"path": "relative.log", "maxBytes": 256 * 1024}, {"path": absolute_path, "maxBytes": 1}, {"path": absolute_path, "maxBytes": 1024 * 1024}, {"path": absolute_path, "maxBytes": True}, {"path": absolute_path, "maxBytes": [256 * 1024]}, {"path": absolute_path, "maxBytes": 256 * 1024, "extra": True}, True]:
            with self.assertRaises(ValueError):
                _normalize_bridge_config({**base, "bridge": {**base["bridge"], "diagnostics": invalid}})

    def test_version_one_shape_passes_through_and_others_fail(self):
        self.assertEqual(_normalize_bridge_config({"version": 1, "host": "::1", "port": 9765, "secretFile": "/tmp/s"}), {"version": 1, "host": "::1", "port": 9765, "secretFile": "/tmp/s"})
        with self.assertRaises(ValueError):
            _normalize_bridge_config({"version": 1, "host": "127.0.0.1", "port": 9765, "secretFile": "/tmp/s", "timeoutMs": 5000})
        with self.assertRaises(ValueError):
            _normalize_bridge_config("not-a-dict")


def fake_status_result():
    return {"connected": False, "adapter": "unavailable", "epoch": None, "protocol": "ableton-live/v1", "registryHash": operation_registry()[1], "operations": ["status", "snapshot", "discover", "get", "reconnect", "session.playback"], "capabilities": []}


def _protect_windows_owner_only(path):
    if os.name != "nt": return
    encoded = base64.b64encode(str(path).encode("utf-8")).decode("ascii")
    script = (
        "$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATH));"
        "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;"
        "$a=if([IO.Directory]::Exists($p)){New-Object System.Security.AccessControl.DirectorySecurity}else{New-Object System.Security.AccessControl.FileSecurity};"
        "$a.SetOwner($sid);$a.SetAccessRuleProtection($true,$false);"
        "$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);"
        "[void]$a.AddAccessRule($rule);"
        "if([IO.Directory]::Exists($p)){[IO.Directory]::SetAccessControl($p,$a)}else{[IO.File]::SetAccessControl($p,$a)}"
    )
    environment = dict(os.environ); environment["ABLETON_MCP_ACL_PATH"] = encoded
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], check=True, env=environment)


class DiagnosticsSecurityTests(unittest.TestCase):
    def test_predictable_temporary_sentinel_cannot_enable_diagnostics(self):
        self.assertFalse(hasattr(remote_module, "_DEBUG_LOG"))
        self.assertFalse(hasattr(remote_module, "_DEBUG_ENABLED"))
        _set_diagnostics_sink(None)
        _debug_trace("dispatch-failure")

    def _owner_file(self, directory, name="bridge-diagnostics.log"):
        root = Path(directory); root.chmod(0o700); _protect_windows_owner_only(root)
        path = root / name; path.write_bytes(b""); path.chmod(0o600); _protect_windows_owner_only(path)
        return path

    def _sink(self, path, *, start_writer=True):
        return _DiagnosticsSink(str(path), start_writer=start_writer, security_validator=_diagnostics_path_safe)

    def test_structured_diagnostics_are_explicit_asynchronous_and_redacted(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self._owner_file(directory)
            self.assertTrue(_diagnostics_path_safe(path))
            sink = self._sink(path); self.assertTrue(sink.enabled)
            _set_diagnostics_sink(sink)
            try:
                try:
                    raise RuntimeError("SECRET-CANARY /Users/example/Project.als browser-query token mac pcm")
                except RuntimeError:
                    _debug_trace("dispatch-failure")
                self.assertTrue(sink.flush_for_test())
                logged = path.read_text(encoding="utf-8")
                self.assertIn('"event":"dispatch-failure"', logged)
                for forbidden in ["SECRET-CANARY", "Project.als", "browser-query", "token", "Traceback", str(path)]:
                    self.assertNotIn(forbidden, logged)
            finally:
                _clear_diagnostics_sink(sink)
            self.assertTrue(sink.wait_closed_for_test())

    def test_thread_start_failure_and_prefilled_oversize_file_fail_safe(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self._owner_file(directory)
            path.write_bytes(b"sensitive-canary" * (remote_module._DIAGNOSTICS_MAX_BYTES // 16 + 1))
            bounded = self._sink(path, start_writer=False)
            self.assertTrue(bounded.enabled); self.assertEqual(path.stat().st_size, 0)
            bounded.close()
            # A configuration from before the bound went up keeps its own, smaller bound.
            path.write_bytes(b"sensitive-canary" * 30000)
            legacy = _DiagnosticsSink(str(path), remote_module._DIAGNOSTICS_LEGACY_MAX_BYTES, start_writer=False, security_validator=_diagnostics_path_safe)
            self.assertTrue(legacy.enabled); self.assertEqual(path.stat().st_size, 0)
            legacy.close()
            self.assertFalse(_DiagnosticsSink(str(path), 1024 * 1024, start_writer=False, security_validator=_diagnostics_path_safe).enabled)
            # Patch only after replacing the Windows validator with an in-process
            # equivalent. subprocess.capture_output also starts helper threads on
            # Windows, and globally failing those would not exercise the writer.
            with patch("ableton_mcp_remote_script.threading.Thread.start", side_effect=RuntimeError("thread unavailable")):
                unavailable = _DiagnosticsSink(str(path), security_validator=lambda candidate: candidate == path)
            self.assertFalse(unavailable.enabled); self.assertIsNone(unavailable._fd)

    @unittest.skipIf(os.name == "nt", "POSIX link and FIFO contract")
    def test_symlink_hardlink_fifo_and_insecure_mode_are_rejected_without_opening(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); root.chmod(0o700)
            target = self._owner_file(directory, "target.log")
            link = root / "link.log"; link.symlink_to(target)
            self.assertFalse(self._sink(link).enabled)
            hard = root / "hard.log"; os.link(target, hard)
            self.assertFalse(self._sink(target).enabled)
            hard.unlink(); target.chmod(0o644)
            self.assertFalse(self._sink(target).enabled)
            fifo = root / "fifo"; os.mkfifo(fifo, 0o600)
            started = time.monotonic(); self.assertFalse(self._sink(fifo).enabled)
            self.assertLess(time.monotonic() - started, 1.0)

    def test_nonblocking_queue_and_fixed_file_bound(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self._owner_file(directory)
            queued = self._sink(path, start_writer=False); self.assertTrue(queued.enabled)
            started = time.monotonic()
            for _ in range(1000): queued.record("capture-tick-failure")
            self.assertLess(time.monotonic() - started, 1.0)
            self.assertEqual(queued._queue.qsize(), 64); self.assertGreater(queued._dropped, 0)
            queued.close()
            bounded = self._sink(path, start_writer=False); self.assertTrue(bounded.enabled)
            try:
                # One near-boundary write proves rotation without launching the
                # Windows security verifier thousands of times.
                self.assertIsNotNone(bounded._fd)
                os.write(bounded._fd, b"x" * (remote_module._DIAGNOSTICS_MAX_BYTES - 1))
                bounded._write((1, "realtime-packet-failure", "internal-error"))
                self.assertLessEqual(path.stat().st_size, remote_module._DIAGNOSTICS_MAX_BYTES)
                self.assertNotIn(b"x", path.read_bytes())
            finally: bounded.close()

    def test_path_or_security_drift_and_write_failure_disable_logging_without_touching_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); path = self._owner_file(directory)
            authority = {"valid": True}
            validator = _diagnostics_path_safe if os.name != "nt" else lambda candidate: authority["valid"] and candidate == path
            sink = _DiagnosticsSink(str(path), security_validator=validator); self.assertTrue(sink.enabled)
            try:
                if os.name == "nt":
                    # Windows intentionally prevents renaming an open file. Model
                    # the validator rejecting equivalent DACL/path authority drift.
                    authority["valid"] = False
                else:
                    moved = root / "moved.log"; path.rename(moved)
                    path.write_bytes(b""); path.chmod(0o600)
                sink.record("result-contract-failure")
                self.assertTrue(sink.flush_for_test())
                self.assertFalse(sink.enabled)
                self.assertEqual(path.read_bytes(), b"")
            finally:
                sink.close(); self.assertTrue(sink.wait_closed_for_test())

            failed = self._sink(path); self.assertTrue(failed.enabled)
            try:
                with patch.object(failed, "_write", side_effect=OSError("injected write failure")):
                    failed.record("capture-tick-failure")
                    self.assertTrue(failed.flush_for_test())
                self.assertFalse(failed.enabled)
                self.assertEqual(path.read_bytes(), b"")
            finally:
                failed.close(); self.assertTrue(failed.wait_closed_for_test())

    @unittest.skipIf(os.name == "nt", "POSIX parent mode contract")
    def test_parent_permission_drift_disables_the_writer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); path = self._owner_file(directory)
            sink = self._sink(path); self.assertTrue(sink.enabled)
            root.chmod(0o777)
            try:
                sink.record("capture-tick-failure")
                self.assertTrue(sink.flush_for_test())
                self.assertFalse(sink.enabled)
                self.assertEqual(path.read_bytes(), b"")
            finally:
                root.chmod(0o700); sink.close()


class RemoteScriptTests(unittest.TestCase):
    def test_windows_security_uses_dacl_not_synthetic_posix_mode_bits(self):
        class SyntheticWindowsPath:
            def stat(self):
                raise AssertionError("Windows mode bits must not be consulted after DACL validation")
        with patch("AbletonMcpBridge.os.name", "nt"):
            self.assertTrue(_mode_owner_only(SyntheticWindowsPath()))
        if os.name != "nt":
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory, "mode-test")
                path.write_text("test", encoding="utf-8")
                path.chmod(0o600); self.assertTrue(_mode_owner_only(path))
                path.chmod(0o644); self.assertFalse(_mode_owner_only(path))

    def test_security_sensitive_files_require_current_owner(self):
        if os.name == "nt":
            # actions/checkout may assign source files to the runner service
            # account. Exercise the real contract with a file created by the
            # current process, as setup does for bridge configuration files.
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory, "bridge-config.json")
                path.write_text("{}", encoding="utf-8")
                encoded = base64.b64encode(str(path).encode("utf-8")).decode("ascii")
                script = "$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATH));$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;$a=New-Object System.Security.AccessControl.FileSecurity;$a.SetOwner($sid);$a.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.File]::SetAccessControl($p,$a)"
                environment = dict(os.environ); environment["ABLETON_MCP_ACL_PATH"] = encoded
                subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], check=True, env=environment)
                self.assertTrue(_owner_controlled(path))
                with patch("AbletonMcpBridge._windows_owner_controlled", return_value=False):
                    self.assertFalse(_owner_controlled(path))
        else:
            path = Path(__file__).resolve()
            self.assertTrue(_owner_controlled(path))
            with patch("AbletonMcpBridge.os.getuid", return_value=path.stat().st_uid + 1):
                self.assertFalse(_owner_controlled(path))

    def test_windows_owner_check_rests_on_the_acl_where_ctypes_is_missing(self):
        # Live's own Python on Windows comes without ctypes: the native comparison can't be made there.
        package = __import__("AbletonMcpBridge")
        path = Path(__file__).resolve()
        with patch.dict(sys.modules, {"ctypes": None}):
            self.assertIsNone(package._windows_owner_controlled(path))
            for verdict in (True, False):
                with patch("AbletonMcpBridge.os.name", "nt"), patch("AbletonMcpBridge._windows_acl_owner_only", return_value=verdict) as acl:
                    self.assertEqual(_owner_controlled(path), verdict)
                    acl.assert_called_once_with(path)

    def test_windows_acl_check_runs_windows_powershell_by_its_full_path_without_a_window(self):
        package = __import__("AbletonMcpBridge")
        seen = {}
        def run(args, **kwargs):
            seen["args"], seen["kwargs"] = args, kwargs
            return types.SimpleNamespace(returncode=0)
        with patch("AbletonMcpBridge.subprocess.run", run), patch.dict(os.environ, {"SYSTEMROOT": r"D:\Windows"}):
            self.assertTrue(package._windows_acl_owner_only(Path("C:/Kumi/bridge-reference.json")))
        self.assertEqual(seen["args"][0], os.path.join(r"D:\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"))
        self.assertEqual(seen["kwargs"]["creationflags"], getattr(subprocess, "CREATE_NO_WINDOW", 0))

    def surface_with_timer(self, serve=None):
        """A Control Surface whose Live has a timer (Live.Base.Timer), the timers it made, its bridge."""
        package = __import__("AbletonMcpBridge"); made = []
        class Timer:
            def __init__(self, callback, interval, repeat):
                self.callback, self.interval, self.repeat, self.running = callback, interval, repeat, False; made.append(self)
            def start(self): self.running = True
            def stop(self): self.running = False
        class Bridge:
            between_ticks = False; served = 0; disconnected = False
            def serve_between_ticks(self):
                self.served += 1
                if serve is not None: serve()
            def disconnect(self): self.disconnected = True
        live = types.ModuleType("Live"); live.Base = types.SimpleNamespace(Timer=Timer)
        surface = object.__new__(package.AbletonMcpBridge)
        surface._bridge, surface._disconnected, surface._willington, surface._timer, surface._scheduled = Bridge(), False, None, None, None
        logged = []; surface.log_message = logged.append
        with patch.dict(sys.modules, {"Live": live}): surface._start_timer()
        return surface, made, logged

    def test_lives_timer_serves_the_bridge_between_ticks_until_disconnect(self):
        surface, made, _ = self.surface_with_timer()
        timer = made[0]
        self.assertEqual((timer.interval, timer.repeat, timer.running, surface._bridge.between_ticks), (1, True, True, True))
        timer.callback(); timer.callback()
        self.assertEqual(surface._bridge.served, 2)
        surface.disconnect()
        self.assertEqual((timer.running, surface._bridge.between_ticks, surface._bridge.disconnected), (False, False, True))
        timer.callback()
        self.assertEqual(surface._bridge.served, 2, "a late timer callback doesn't touch the bridge")

    def test_a_timer_that_fails_stops_and_the_ticks_serve_as_before(self):
        def fail(): raise RuntimeError("boom")
        surface, made, logged = self.surface_with_timer(fail)
        made[0].callback(); made[0].callback()
        self.assertEqual(surface._bridge.served, 1, "stopped after its first failure")
        self.assertEqual((made[0].running, surface._bridge.between_ticks), (False, False))
        self.assertEqual(len(logged), 1)
        self.assertTrue(logged[0].startswith("Bridge timer stopped (RuntimeError: boom); display ticks serve the bridge\nTraceback"), logged[0])

    def test_the_timers_rate_is_said_once(self):
        package = __import__("AbletonMcpBridge")
        with patch.object(package.time, "perf_counter", return_value=100.0): surface, made, logged = self.surface_with_timer()
        for at in (101.0, 102.0, 103.0, 104.0, 105.0, 106.0):
            with patch.object(package.time, "perf_counter", return_value=at): made[0].callback()
        self.assertEqual(logged, ["Bridge timer: 1 calls a second"])

    def test_a_live_without_a_timer_is_served_by_its_ticks(self):
        package = __import__("AbletonMcpBridge")
        surface = object.__new__(package.AbletonMcpBridge)
        surface._bridge = types.SimpleNamespace(between_ticks=False); surface._timer = None
        with patch.dict(sys.modules, {"Live": None}): surface._start_timer()
        self.assertEqual((surface._timer, surface._bridge.between_ticks), (None, False))

    def test_scheduled_callback_does_not_touch_bridge_after_disconnect(self):
        surface = object.__new__(__import__("AbletonMcpBridge").AbletonMcpBridge)
        surface._disconnected = True

        class Bridge:
            def __init__(self):
                self.calls = 0

            def update_display(self):
                self.calls += 1

        surface._bridge = Bridge()
        surface._drain()
        self.assertEqual(surface._bridge.calls, 0)

    def test_authentication_and_replay_protection(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: fake_status_result())
        unsigned = remote.bound({"version": PROTOCOL, "id": "one", "method": "status", "nonce": "0000000000000001", "sequence": 1})
        request = {**unsigned, "mac": remote.sign(unsigned)}
        self.assertTrue(remote.dispatch(request)["ok"])
        self.assertFalse(remote.dispatch(request)["ok"])

    def test_channel_binding_rejects_cross_connection_and_bridge_epoch_replay(self):
        secret = "0123456789abcdef0123456789abcdef"
        first = AuthenticatedRemoteScript(secret, lambda method, request: {"connected": False, "adapter": "unavailable", "epoch": None, "protocol": "ableton-live/v1", "registryHash": operation_registry()[1], "operations": ["status", "snapshot", "discover", "get", "reconnect", "session.playback"]}, "bridge-epoch-0000000000000001", "connection-one-0000000000001")
        second = AuthenticatedRemoteScript(secret, first._operation, "bridge-epoch-0000000000000001", "connection-two-0000000000002")
        restarted = AuthenticatedRemoteScript(secret, first._operation, "bridge-epoch-0000000000000002", "connection-one-0000000000001")
        unsigned = first.bound({"version": PROTOCOL, "id": "bound", "method": "status", "nonce": "bound-nonce-00001", "sequence": 1})
        frame = {**unsigned, "mac": first.sign(unsigned)}
        self.assertTrue(first.dispatch(frame)["ok"])
        self.assertFalse(second.dispatch(frame)["ok"])
        self.assertFalse(restarted.dispatch(frame)["ok"])

    def test_sequence_must_be_positive_and_safe(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: method)
        for sequence in (0, -1, 2**53, 2**53 + 1):
            unsigned = remote.bound({"version": PROTOCOL, "id": "sequence", "method": "status", "nonce": "sequence-nonce-0001", "sequence": sequence})
            self.assertFalse(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})["ok"])

    def test_operation_failures_are_wire_errors(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: (_ for _ in ()).throw(RuntimeError("not available")))
        unsigned = remote.bound({"version": PROTOCOL, "id": "one", "method": "snapshot", "nonce": "0000000000000001", "sequence": 1})
        result = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"], "request failed: RuntimeError: not available", "Live's own reason comes through")
        validation = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: (_ for _ in ()).throw(ValueError("structure changed since preview\x1b[2J")))
        unsigned = validation.bound({"version": PROTOCOL, "id": "two", "method": "snapshot", "nonce": "0000000000000002", "sequence": 1})
        self.assertEqual(validation.dispatch({**unsigned, "mac": validation.sign(unsigned)})["error"], "request failed: structure changed since preview [2J")

    def test_result_schema_violation_invalidates_authenticated_channel(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: {"connected": "yes"})
        unsigned = remote.bound({"version": PROTOCOL, "id": "schema", "method": "status", "nonce": "schema-nonce-0001", "sequence": 1})
        result = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertFalse(result["ok"]); self.assertEqual(result["error"], "response contract failed"); self.assertTrue(remote.invalid)

    def test_discovery_filter_registry_values_are_bounded_scalars(self):
        validate_operation_payload("discover", "request", {"kind": "track", "filters": {"name": "Bass", "armed": False, "index": 2, "parentRef": None}})
        with self.assertRaises(ValueError): validate_operation_payload("discover", "request", {"kind": "track", "filters": {"nested": {}}})
        with self.assertRaises(ValueError): validate_operation_payload("discover", "request", {"kind": "track", "filters": {"name": "x" * 257}})

    def test_random_ordered_nonces_and_unknown_fields(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: fake_status_result())
        first = remote.bound({"version": PROTOCOL, "id": "one", "method": "status", "nonce": "zzzzzzzzzzzzzzzz1", "sequence": 1})
        second = remote.bound({"version": PROTOCOL, "id": "two", "method": "status", "nonce": "aaaaaaaaaaaaaaaa2", "sequence": 2})
        self.assertTrue(remote.dispatch({**first, "mac": remote.sign(first)})["ok"])
        self.assertTrue(remote.dispatch({**second, "mac": remote.sign(second)})["ok"])
        extra = {**second, "id": "three", "nonce": "bbbbbbbbbbbbbbbb3", "unexpected": True}
        self.assertFalse(remote.dispatch({**extra, "mac": remote.sign(extra)})["ok"])

    def test_authenticated_retirement_is_bounded_and_transaction_scoped(self):
        calls = []
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: calls.append((method, request.get("transactionId"), request.get("terminal"))) or {"retired": 2})
        unsigned = remote.bound({"version": PROTOCOL, "id": "retire-one", "method": "retire", "transactionId": "transaction-1234", "terminal": True, "nonce": "retire-nonce-0001", "sequence": 1})
        result = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertTrue(result["ok"]); self.assertEqual(result["result"], {"retired": 2}); self.assertEqual(calls, [("retire", "transaction-1234", True)])
        invalid = remote.bound({"version": PROTOCOL, "id": "retire-two", "method": "retire", "transactionId": "short", "nonce": "retire-nonce-0002", "sequence": 2})
        self.assertFalse(remote.dispatch({**invalid, "mac": remote.sign(invalid)})["ok"])

    def test_unknown_method_is_rejected_before_operation(self):
        called = []
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: called.append(method))
        request = remote.bound({"version": PROTOCOL, "id": "one", "method": "delete", "nonce": "cccccccccccccccc4", "sequence": 1})
        self.assertFalse(remote.dispatch({**request, "mac": remote.sign(request)})["ok"])
        self.assertEqual(called, [])

    def test_malformed_requests_are_wire_errors_and_nonces_are_bounded(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: method)
        self.assertFalse(remote.dispatch(None)["ok"])
        unsigned = remote.bound({"version": PROTOCOL, "id": "large", "method": "status", "nonce": "x" * 257, "sequence": 1})
        self.assertFalse(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})["ok"])

    def test_malformed_frame_error_is_authenticated_and_redacted(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: method)
        response = remote.error_response()
        unsigned = {key: value for key, value in response.items() if key != "mac"}
        self.assertEqual(response["mac"], remote.sign(unsigned))
        self.assertEqual(response["error"], "malformed request")
        self.assertNotIn("Traceback", response["error"])

    def test_wire_signing_rejects_oversized_and_deep_values(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: method)
        oversized = {"version": PROTOCOL, "id": "large", "method": "invoke", "operation": "browser.search", "args": {"query": "x" * (remote_module.MAX_WIRE_STRING_LENGTH + 1)}, "nonce": "large-wire-value-0001", "sequence": 1}
        with self.assertRaises(ValueError):
            remote.sign(oversized)
        nested = "value"
        for _ in range(remote_module.MAX_WIRE_DEPTH + 1):
            nested = {"value": nested}
        deeply_nested = {"version": PROTOCOL, "id": "deep", "method": "invoke", "operation": "browser.search", "args": nested, "nonce": "deep-wire-value-0001", "sequence": 1}
        with self.assertRaises(ValueError):
            remote.sign(deeply_nested)
        with patch.object(remote_module, "MAX_WIRE_ARRAY_LENGTH", 512):
            remote.sign({"version": PROTOCOL, "id": "bounded-array", "method": "status", "values": list(range(512))})
            with self.assertRaises(ValueError):
                remote.sign({"version": PROTOCOL, "id": "oversized-array", "method": "status", "values": list(range(513))})

    def test_operation_names_with_digits_reach_the_mapper(self):
        # EQ Eight's operation is eq8.set: a name with a digit must pass the wire's name check.
        seen = []
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: seen.append(request["operation"]) or {"preflightToken": "p" * 24, "confirmation": "c" * 24, "operation": request["operation"], "argsDigest": "a" * 64, "stateDigest": "b" * 64, "impact": "mutates-live", "expiresAt": 1})
        unsigned = remote.bound({"version": PROTOCOL, "id": "eq8", "method": "preflight", "operation": "eq8.set", "args": {}, "nonce": "digit-operation-0001", "sequence": 1, "transactionId": "transaction-eq8"})
        response = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertNotEqual(response.get("error"), "operation is required")
        self.assertEqual(seen, ["eq8.set"])

    def test_direct_authenticated_mutation_without_prepared_authority_is_rejected(self):
        calls = []
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: calls.append((method, request["operation"])))
        attempts = [("session.emergency-stop", {"expectedTargets": [], "expectedRecording": "stopped"}), ("clip.delete", {"ref": "clip:x"}), ("track.delete", {"ref": "track:x"}), ("scene.delete", {"ref": "scene:x"}), ("note.add", {"ref": "clip:x", "note": {}}), ("device.delete", {"ref": "device:x"}), ("device.parameter.set", {"ref": "parameter:x", "value": 0.5, "expectedRevision": 1})]
        for sequence, (operation, args) in enumerate(attempts, 1):
            unsigned = remote.bound({"version": PROTOCOL, "id": f"invoke-{sequence}", "method": "invoke", "operation": operation, "args": args, "nonce": f"invoke-nonce-{sequence:04d}", "sequence": sequence})
            self.assertFalse(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})["ok"])
        self.assertEqual(calls, [])

    def test_invoke_rejects_unbounded_or_malformed_arguments(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: method)
        unsigned = remote.bound({"version": PROTOCOL, "id": "invoke", "method": "invoke", "operation": "invalid", "args": {}, "nonce": "invoke-nonce-0002", "sequence": 1})
        self.assertFalse(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})["ok"])


class FakeClip:
    def __init__(self, length):
        self.length = length
        self.name = ""
        self.notes = []
        self.next_note_id = 1

    def add_new_notes(self, notes):
        for note in notes:
            value = dict(note) if isinstance(note, dict) else {"pitch": note.pitch, "start_time": note.start_time, "duration": note.duration, "velocity": note.velocity, "mute": getattr(note, "mute", False), "probability": getattr(note, "probability", 1.0), "velocity_deviation": getattr(note, "velocity_deviation", 0.0), "release_velocity": getattr(note, "release_velocity", 64.0)}
            value["note_id"] = self.next_note_id; self.next_note_id += 1; self.notes.append(value)

    def get_notes(self, *_): return list(self.notes)
    def get_all_notes_extended(self): return list(self.notes)
    def remove_notes_by_id(self, ids): self.notes = [note for note in self.notes if note.get("note_id") not in set(ids)]


class FakeSlot:
    def __init__(self):
        self.clip = None

    def create_clip(self, length):
        self.clip = FakeClip(length)
        return self.clip

    def delete_clip(self):
        self.clip = None


class FakeTrack:
    has_midi_input = True

    def __init__(self):
        self.name = "Drums"
        self.arm = False
        self.current_monitoring_state = 2
        self.playing_slot_index = -1
        self.fired_slot_index = -1
        self.clip_slots = [FakeSlot()]
        self.devices = [FakeDevice()]


class FakeParameter:
    def __init__(self):
        self.name = "Gain"
        self.value = 0.5
        self.min = 0.0
        self.max = 1.0
        self.quantization = 0.25
        self.is_quantized = True
        self.enabled = True
        self.is_enabled = True
        self.automatable = True


class FakeDevice:
    def __init__(self):
        self.name = "Utility"
        self.class_name = "AudioEffectUtility"
        self.enabled = True
        self.parameters = [FakeParameter()]


class FakeScene:
    def __init__(self, name="Scene 1"):
        self.name = name


class FakeSong:
    def __init__(self):
        self.tracks = [FakeTrack()]
        self.return_tracks = []
        self.master_track = None
        self.scenes = [FakeScene()]
        self.is_playing = False
        self.record_mode = False
        self.session_record = False
        self.current_song_time = 0.0
        self.clip_trigger_quantization = "1_bar"

    def create_midi_track(self, index):
        track = FakeTrack()
        track.name = "MIDI Track"
        self.tracks.insert(index, track)
        return track

    def create_audio_track(self, index):
        track = FakeTrack()
        track.name = "Audio Track"
        track.has_midi_input = False
        self.tracks.insert(index, track)
        return track

    def create_scene(self, index):
        scene = FakeScene()
        self.scenes.insert(index, scene)
        return scene

    def delete_track(self, index):
        self.tracks.pop(index)

    def delete_scene(self, index):
        self.scenes.pop(index)


class FakeLocator:
    def __init__(self, time, name=""):
        self.time = time
        self.name = name


class FakeAuditionSong(FakeSong):
    def __init__(self):
        super().__init__()
        song = self
        scene = FakeScene("Scene 1")

        def fire():
            song.is_playing = True
            song.tracks[0].playing_slot_index = 0
            song.tracks[0].fired_slot_index = 0

        scene.fire = fire
        self.scenes = [scene]
        self.tracks[0].clip_slots[0].clip = FakeClip(4.0)
        self.stopped_all = 0

    def stop_all_clips(self):
        self.stopped_all += 1
        for track in self.tracks:
            track.playing_slot_index = -1
            track.fired_slot_index = -1

    def stop_playing(self):
        self.is_playing = False


class FakeRouteChoice:
    def __init__(self, name):
        self.name = name
        self.display_name = name


class FakeCapturedAudioClip(FakeClip):
    def __init__(self):
        super().__init__(2.0)
        self.name = "MCP Ephemeral Capture"
        self.is_audio_clip = True
        self.file_path = None
        self.is_recording = True
        self.gain = 1.0


class FakeCaptureSlot(FakeSlot):
    def __init__(self, fire_callback):
        super().__init__()
        self._fire_callback = fire_callback

    def fire(self):
        self._fire_callback(self)


class FakeCaptureTrack:
    def __init__(self, song, name, audio=False):
        self.song = song
        self.name = name
        self.has_audio_input = audio
        self.has_midi_input = not audio
        self.can_be_armed = True
        self.arm = False
        self.current_monitoring_state = 2
        self.playing_slot_index = -1
        self.fired_slot_index = -1
        self.devices = []
        self.available_input_routing_types = [FakeRouteChoice("Ext. In"), FakeRouteChoice("Resampling")] if audio else []
        self.input_routing_type = self.available_input_routing_types[0] if audio else None
        self.current_input_routing = self.input_routing_type
        def fire(slot):
            self.song.is_playing = True
            self.playing_slot_index = 0
            self.fired_slot_index = 0
            if self.has_audio_input and self.arm and slot.clip is None:
                slot.clip = FakeCapturedAudioClip()
                slot.clip.name = self.name
        self.clip_slots = [FakeCaptureSlot(fire)]

    def stop_all_clips(self, *_):
        self.playing_slot_index = -1
        self.fired_slot_index = -1
        for slot in self.clip_slots:
            if isinstance(slot.clip, FakeCapturedAudioClip):
                slot.clip.is_recording = False
                slot.clip.file_path = "/tmp/MCP Ephemeral Capture.wav"


class FakeCaptureSong(FakeSong):
    def __init__(self):
        self.name = "MCP-Audition-Disposable"
        self.return_tracks = []
        self.master_track = None
        self.scenes = [FakeScene("Scene 1")]
        self.is_playing = False
        self.record_mode = False
        self.session_record = False
        self.current_song_time = 7.0
        self.clip_trigger_quantization = 4
        self.tracks = [FakeCaptureTrack(self, "Source", audio=False), FakeCaptureTrack(self, "Capture", audio=True)]
        self.tracks[0].clip_slots[0].clip = FakeClip(4.0)

    def stop_playing(self):
        self.is_playing = False


class FakeArrangementSong(FakeSong):
    def __init__(self):
        super().__init__()
        self.cue_points = [FakeLocator(0, "Intro")]

    def set_or_delete_cue(self):
        # Like Live: no arguments; toggles a locator at the playhead.
        position = self.current_song_time
        for index, locator in enumerate(self.cue_points):
            if locator.time == position:
                self.cue_points.pop(index)
                return
        self.cue_points.append(FakeLocator(position))


class FakeInstance:
    def __init__(self):
        self.song = FakeSong()


class ControlSurfaceTests(unittest.TestCase):
    def test_registry_is_canonical_and_hashed(self):
        registry, digest = operation_registry()
        self.assertEqual(registry["protocol"], "ableton-live/v1")
        canonical = json.dumps(registry, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
        self.assertEqual(digest, hashlib.sha256(canonical).hexdigest())
        self.assertEqual(digest, "ec05dd401ec098adb77da1c185aff1857be2bd87859afe9dda4bfeb14e04aa57")
        self.assertIn("audio.capture.start", [item["id"] for item in registry["operations"]])
        self.assertIn("device.parameter.set", [item["id"] for item in registry["operations"]])
        ids = [item["id"] for item in registry["operations"]]
        self.assertNotIn("scene.launch", ids)
        reserved = {"project.save", "arrangement.automation.create", "audio.warp-marker.add", "audio.take-lane.read", "audio.comp.read", "browser.preview.start"}
        self.assertTrue(reserved <= set(ids)); self.assertTrue(reserved.isdisjoint(LiveObjectMapper(FakeSong()).status()["operations"]))

    def test_lom_audit_lists_every_live_class_with_its_members(self):
        # Inside Live the submodules are named without the "Live." prefix.
        live = types.ModuleType("Live"); song_module = types.ModuleType("Song"); live.os = os
        class Song:
            """The Live Set."""
            tempo = property(lambda self: 120.0, doc="Tempo in beats per minute")
            def begin_undo_step(self):
                """Opens an undo step."""
            class View:
                selected_track = property(lambda self: None)
        song_module.Song = Song; live.Song = song_module
        mapper = LiveObjectMapper(FakeSong())
        with patch.dict(sys.modules, {"Live": None}): self.assertFalse(mapper._operation_supported("dev.lom-audit"))
        with patch.dict(sys.modules, {"Live": live}):
            self.assertTrue(mapper._operation_supported("dev.lom-audit"))
            result = mapper.invoke("dev.lom-audit", {})
        validate_operation_payload("dev.lom-audit", "result", result)
        by_path = {row["path"]: row for row in result["classes"]}
        members = {row["name"]: row for row in by_path["Live.Song.Song"]["members"]}
        self.assertEqual(members["tempo"]["kind"], "property"); self.assertEqual(members["tempo"]["doc"], "Tempo in beats per minute")
        self.assertEqual(members["begin_undo_step"]["kind"], "method"); self.assertEqual(members["View"]["kind"], "class")
        self.assertIn("selected_track", [row["name"] for row in by_path["Live.Song.Song.View"]["members"]])
        self.assertEqual(by_path["Live.Song.Song"]["doc"], "The Live Set.")
        self.assertFalse(any(path.startswith("Live.os") for path in by_path))

    def test_status_result_carries_every_canonical_operation_within_the_bound(self):
        registry, _ = operation_registry()
        ids = [item["id"] for item in registry["operations"]]
        self.assertLessEqual(len(ids), 4096)
        payload = {"adapter": "remote-script", "connected": True, "epoch": 1, "protocol": "ableton-live/v1", "registryHash": operation_registry()[1], "operations": ids}
        validate_operation_payload("status", "result", payload)

    def test_provenance_is_explicit_and_fake_is_the_direct_default(self):
        self.assertEqual(LiveObjectMapper(FakeSong()).status()["provenance"], "fake-live")
        self.assertEqual(LiveObjectMapper(FakeSong(), provenance="real-live").status()["provenance"], "real-live")
        with self.assertRaises(ValueError): LiveObjectMapper(FakeSong(), provenance="unknown")

    def capture_fixture(self):
        song = FakeCaptureSong(); mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        source = snapshot["tracks"][0]["clipSlots"][0]["ref"]
        destination = snapshot["tracks"][1]["clipSlots"][0]["ref"]
        args = {"setName": song.name, "sourceSlotRef": source, "destinationSlotRef": destination, "outputSafety": {"safe": True, "provenance": "unit-test-operator"}}
        return song, mapper, source, destination, args

    @staticmethod
    def clip_creation_args(mapper, track_ref, scene_index, **values):
        snapshot = mapper.snapshot(); track = next(row for row in snapshot["tracks"] if row["ref"] == track_ref); slot = next(row for row in track["clipSlots"] if row["sceneIndex"] == scene_index); scene = next(row for row in snapshot["scenes"] if row["index"] == scene_index)
        return {**values, "trackRef": track_ref, "sceneIndex": scene_index, "expectedTrackIdentity": track["objectIdentity"], "expectedSlotRef": slot["ref"], "expectedSlotIdentity": slot["objectIdentity"], "expectedSceneRef": scene["ref"], "expectedSceneIdentity": scene["objectIdentity"]}

    @staticmethod
    def note_authority(mapper, clip_ref):
        snapshot = mapper.snapshot(); clip = next(clip for track in snapshot["tracks"] for clip in track["clips"] if clip["ref"] == clip_ref)
        return {"expectedClipAuthority": mapper._session_clip_authority(clip_ref), "expectedNotesRevision": clip["notesRevision"]}

    @staticmethod
    def parameter_authority(mapper, parameter_ref):
        authority = mapper._realtime_parameter_authority(parameter_ref)
        return {"expectedObjectIdentity": authority["parameterIdentity"], "expectedOwnerRef": authority["ownerRef"], "expectedOwnerIdentity": authority["ownerIdentity"], "expectedTrackRef": authority["trackRef"], "expectedTrackIdentity": authority["trackIdentity"], "expectedSiblings": authority["siblings"]}

    def test_resampling_capture_lifecycle_is_fenced_bounded_and_ephemeral(self):
        song, mapper, source, destination, args = self.capture_fixture()
        self.assertIn("audio.capture.resampling", mapper.status()["capabilities"])
        preview = mapper.invoke("audio.capture.inspect", args)
        unsafe = {key: value for key, value in args.items() if key != "outputSafety"}
        with self.assertRaises(ValueError): mapper.invoke("audio.capture.start", {**unsafe, "captureId": "capture-unsafe-output", "fence": preview["fence"], "maxDurationMs": 1000})
        self.assertEqual(preview["captureMode"], "session-slot-resampling")
        self.assertEqual(preview["rawRetention"], "ephemeral")
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-test-0001", "fence": preview["fence"], "maxDurationMs": 1000})
        self.assertEqual(started["state"], "active")
        self.assertTrue(song.tracks[1].arm)
        self.assertEqual(song.tracks[1].input_routing_type.name, "Resampling")
        status = mapper.invoke("audio.capture.status", {})
        self.assertTrue(status["active"])
        self.assertNotIn("token", status)
        stopped = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        self.assertEqual(stopped["state"], "captured")
        self.assertEqual(stopped["clip"]["filePath"], "/tmp/MCP Ephemeral Capture.wav")
        self.assertFalse(song.is_playing)
        self.assertFalse(song.tracks[1].arm)
        self.assertEqual(song.tracks[1].input_routing_type.name, "Ext. In")
        self.assertEqual(song.current_song_time, 7.0)
        cleaned = mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": stopped["clip"]["ref"]})
        self.assertTrue(cleaned["cleaned"])
        self.assertIsNone(song.tracks[1].clip_slots[0].clip)
        self.assertEqual(mapper.invoke("audio.capture.status", {})["state"], "cleaned")

    def test_capture_cleanup_accepts_deleted_then_raised_acknowledgement_loss(self):
        song, mapper, _, _, args = self.capture_fixture(); preview = mapper.invoke("audio.capture.inspect", args); started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-cleanup-ack-loss", "fence": preview["fence"], "maxDurationMs": 1000}); stopped = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]}); reference = stopped["clip"]["ref"]; slot = song.tracks[1].clip_slots[0]; original = slot.delete_clip
        def lost_ack(): original(); raise RuntimeError("injected cleanup acknowledgement loss")
        slot.delete_clip = lost_ack; cleaned = mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": reference}); self.assertTrue(cleaned["cleaned"]); self.assertEqual(mapper.invoke("audio.capture.status", {})["state"], "cleaned")
        with self.assertRaises(KeyError): mapper.refs.get(reference)

    def test_resampling_capture_watchdog_and_independent_emergency_stop(self):
        song, mapper, source, destination, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-test-0002", "fence": preview["fence"], "maxDurationMs": 1000})
        with self.assertRaises(ValueError):
            mapper.invoke("audio.capture.emergency-stop", {"captureId": started["captureId"], "sourceSlotRef": source, "destinationSlotRef": source})
        mapper._capture_state["deadlineMonotonic"] = 0
        mapper.capture_tick()
        status = mapper.invoke("audio.capture.status", {})
        self.assertFalse(status["active"]); self.assertTrue(status["watchdogStopped"]); self.assertEqual(status["state"], "captured")
        emergency = mapper.invoke("audio.capture.emergency-stop", {"captureId": started["captureId"], "sourceSlotRef": source, "destinationSlotRef": destination})
        self.assertTrue(emergency["stopped"])
        mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": status["clip"]["ref"]})
        self.assertFalse(song.is_playing); self.assertFalse(song.tracks[1].arm)

    def test_resampling_capture_refuses_stale_state_and_reports_external_interference(self):
        song, mapper, source, destination, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        song.current_song_time = 8.0
        with self.assertRaises(ValueError):
            mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-test-0003", "fence": preview["fence"], "maxDurationMs": 1000})
        song.current_song_time = 7.0
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-test-0004", "fence": preview["fence"], "maxDurationMs": 1000})
        external = FakeRouteChoice("External Sidechain")
        song.tracks[1].input_routing_type = external
        stopped = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        self.assertIn("destination-route-changed-externally", stopped["residual"])
        self.assertIs(song.tracks[1].input_routing_type, external)
        mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": stopped["clip"]["ref"]})

    def test_capture_binds_an_asynchronously_appearing_recording_clip_only(self):
        song, mapper, _, _, args = self.capture_fixture()
        destination_track = song.tracks[1]; destination_slot = destination_track.clip_slots[0]
        def delayed_fire(_slot):
            song.is_playing = True; destination_track.playing_slot_index = 0; destination_track.fired_slot_index = 0
        destination_slot._fire_callback = delayed_fire
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-delayed-owner", "fence": preview["fence"], "maxDurationMs": 1000})
        self.assertTrue(mapper.invoke("audio.capture.status", {})["active"])
        destination_slot.clip = FakeCapturedAudioClip(); destination_slot.clip.name = destination_track.name
        mapper.capture_tick()
        stopped = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": stopped["clip"]["ref"]})
        self.assertIsNone(destination_slot.clip)

        other_song, other_mapper, _, _, other_args = self.capture_fixture()
        other_track = other_song.tracks[1]; other_slot = other_track.clip_slots[0]
        other_slot._fire_callback = lambda _slot: setattr(other_song, "is_playing", True)
        other_preview = other_mapper.invoke("audio.capture.inspect", other_args)
        other_mapper.invoke("audio.capture.start", {**other_args, "captureId": "capture-unit-delayed-replacement", "fence": other_preview["fence"], "maxDurationMs": 1000})
        replacement = FakeClip(8.0); replacement.name = "USER CLIP"; other_slot.clip = replacement
        other_mapper.capture_tick()
        self.assertIs(other_slot.clip, replacement)
        self.assertEqual(other_mapper.invoke("audio.capture.status", {})["state"], "failed")

    def test_capture_fence_refuses_a_replacement_destination_track_or_slot(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        replacement = FakeCaptureTrack(song, "Replacement Capture", audio=True)
        song.tracks[1] = replacement
        with self.assertRaisesRegex(ValueError, "state changed"):
            mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-replaced-destination", "fence": preview["fence"], "maxDurationMs": 1000})
        self.assertIsNone(replacement.clip_slots[0].clip)

    def test_capture_refuses_a_foreign_recording_clip_without_private_tag(self):
        song, mapper, _, _, args = self.capture_fixture()
        destination_track = song.tracks[1]; destination_slot = destination_track.clip_slots[0]
        destination_slot._fire_callback = lambda _slot: setattr(song, "is_playing", True)
        preview = mapper.invoke("audio.capture.inspect", args)
        mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-foreign-recording", "fence": preview["fence"], "maxDurationMs": 1000})
        foreign = FakeCapturedAudioClip(); foreign.name = "FOREIGN RECORDING"; destination_slot.clip = foreign
        mapper.capture_tick()
        self.assertIs(destination_slot.clip, foreign)
        status = mapper.invoke("audio.capture.status", {})
        self.assertEqual(status["state"], "failed"); self.assertIn("destination-clip-lacks-private-ownership-tag", status["residual"])

    def test_capture_fence_refuses_a_replacement_in_the_same_source_slot(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        replacement = FakeClip(4.0); replacement.name = "UNAUTHORIZED REPLACEMENT"
        song.tracks[0].clip_slots[0].clip = replacement
        with self.assertRaisesRegex(ValueError, "state changed"):
            mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-replaced-source", "fence": preview["fence"], "maxDurationMs": 1000})
        self.assertFalse(song.is_playing)

    def test_capture_cleanup_and_shutdown_never_delete_a_slot_replacement(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-owned-identity", "fence": preview["fence"], "maxDurationMs": 1000})
        stopped = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        replacement = FakeClip(8.0); replacement.name = "USER CLIP"
        song.tracks[1].clip_slots[0].clip = replacement
        with self.assertRaisesRegex(ValueError, "identity"):
            mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": stopped["clip"]["ref"]})
        self.assertIs(song.tracks[1].clip_slots[0].clip, replacement)
        mapper.capture_shutdown()
        self.assertIs(song.tracks[1].clip_slots[0].clip, replacement)
        self.assertEqual(mapper.invoke("audio.capture.status", {})["state"], "failed")

        clean_song, clean_mapper, _, _, clean_args = self.capture_fixture()
        clean_preview = clean_mapper.invoke("audio.capture.inspect", clean_args)
        clean_started = clean_mapper.invoke("audio.capture.start", {**clean_args, "captureId": "capture-unit-post-clean", "fence": clean_preview["fence"], "maxDurationMs": 1000})
        clean_stopped = clean_mapper.invoke("audio.capture.stop", {"captureId": clean_started["captureId"], "token": clean_started["token"]})
        clean_mapper.invoke("audio.capture.cleanup", {"captureId": clean_started["captureId"], "token": clean_started["token"], "expectedClipRef": clean_stopped["clip"]["ref"]})
        post_cleanup = FakeClip(8.0); post_cleanup.name = "POST CLEANUP USER CLIP"
        clean_song.tracks[1].clip_slots[0].clip = post_cleanup
        clean_mapper.capture_shutdown()
        self.assertIs(clean_song.tracks[1].clip_slots[0].clip, post_cleanup)

    def test_destination_fire_that_schedules_then_raises_stays_recoverable(self):
        song, mapper, source, destination, args = self.capture_fixture()
        destination_track = song.tracks[1]; destination_slot = destination_track.clip_slots[0]
        def schedule_then_raise(_slot):
            song.is_playing = True; destination_track.playing_slot_index = 0; destination_track.fired_slot_index = 0
            raise RuntimeError("fire raised after scheduling")
        destination_slot._fire_callback = schedule_then_raise
        preview = mapper.invoke("audio.capture.inspect", args)
        with self.assertRaisesRegex(RuntimeError, "after scheduling"):
            mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-fire-raised", "fence": preview["fence"], "maxDurationMs": 1000})
        failed = mapper.invoke("audio.capture.status", {})
        self.assertEqual(failed["state"], "failed"); self.assertIsInstance(failed["recoveryToken"], str)
        late = FakeCapturedAudioClip(); late.name = destination_track.name; destination_slot.clip = late
        mapper.capture_tick()
        observed = mapper.invoke("audio.capture.status", {})
        self.assertTrue(observed["active"]); self.assertNotEqual(observed["state"], "cleaned")
        stopped = mapper.invoke("audio.capture.emergency-stop", {"captureId": failed["captureId"], "sourceSlotRef": source, "destinationSlotRef": destination})
        mapper.invoke("audio.capture.cleanup", {"captureId": failed["captureId"], "token": failed["recoveryToken"], "expectedClipRef": stopped["clip"]["ref"]})
        self.assertIsNone(destination_slot.clip)

    def test_partial_start_failure_and_shutdown_preserve_owned_clip_recovery_identity(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        song.tracks[0].clip_slots[0]._fire_callback = lambda _slot: (_ for _ in ()).throw(RuntimeError("injected source fire failure"))
        with self.assertRaisesRegex(RuntimeError, "injected source fire failure"):
            mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-partial-start", "fence": preview["fence"], "maxDurationMs": 1000})
        status = mapper.invoke("audio.capture.status", {})
        self.assertIsNotNone(song.tracks[1].clip_slots[0].clip); self.assertIsInstance(status.get("recoveryToken"), str)
        self.assertNotEqual(status["state"], "cleaned")

        other_song, other_mapper, source, destination, other_args = self.capture_fixture()
        other_preview = other_mapper.invoke("audio.capture.inspect", other_args)
        started = other_mapper.invoke("audio.capture.start", {**other_args, "captureId": "capture-unit-shutdown-preserve", "fence": other_preview["fence"], "maxDurationMs": 1000})
        stopped = other_mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        owned = other_song.tracks[1].clip_slots[0].clip
        other_mapper.capture_shutdown()
        self.assertIs(other_song.tracks[1].clip_slots[0].clip, owned)
        shutdown = other_mapper.invoke("audio.capture.status", {})
        self.assertEqual(shutdown["state"], "failed"); self.assertIn("bridge-shutdown-requires-host-or-manual-media-cleanup", shutdown["residual"])
        self.assertEqual(stopped["clip"]["ref"], shutdown["clip"]["ref"])

    def test_capture_retries_when_owned_clip_is_still_recording_despite_stopped_playback(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-recording-retry", "fence": preview["fence"], "maxDurationMs": 1000})
        destination = song.tracks[1]; calls = {"count": 0}
        def incomplete_stop(*_args):
            calls["count"] += 1; destination.playing_slot_index = -1; destination.fired_slot_index = -1
        destination.stop_all_clips = incomplete_stop
        mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        before = calls["count"]
        mapper.capture_tick(); mapper.capture_tick()
        status = mapper.invoke("audio.capture.status", {})
        self.assertGreater(calls["count"], before); self.assertTrue(status["active"]); self.assertTrue(destination.clip_slots[0].clip.is_recording)

    def test_capture_unknown_owned_recording_state_never_finalizes_as_stopped(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-recording-unknown", "fence": preview["fence"], "maxDurationMs": 1000})
        destination = song.tracks[1]
        def unreadable_stop(*_args):
            destination.playing_slot_index = -1; destination.fired_slot_index = -1
            if hasattr(destination.clip_slots[0].clip, "is_recording"): del destination.clip_slots[0].clip.is_recording
        destination.stop_all_clips = unreadable_stop
        mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        mapper.capture_tick(); status = mapper.invoke("audio.capture.status", {})
        self.assertTrue(status["active"]); self.assertTrue(status["unsafe"]); self.assertNotEqual(status["state"], "captured")

    def test_capture_failed_stop_remains_active_and_watchdog_retryable(self):
        song, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        started = mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-stop-retry", "fence": preview["fence"], "maxDurationMs": 1000})
        original_stop = song.stop_playing
        def injected_failure():
            raise RuntimeError("injected stop failure")
        song.stop_playing = injected_failure
        immediate = mapper.invoke("audio.capture.stop", {"captureId": started["captureId"], "token": started["token"]})
        self.assertTrue(immediate["stopped"]); self.assertFalse(immediate["playbackStopped"])
        failed = mapper.invoke("audio.capture.status", {})
        self.assertTrue(failed["active"]); self.assertTrue(failed["unsafe"]); self.assertFalse(failed["playbackStopped"]); self.assertIn(failed["state"], {"failed", "captured"})
        song.stop_playing = original_stop
        mapper.capture_tick()
        recovered = mapper.invoke("audio.capture.status", {})
        self.assertFalse(recovered["active"]); self.assertTrue(recovered["playbackStopped"]); self.assertEqual(recovered["state"], "captured")
        mapper.invoke("audio.capture.cleanup", {"captureId": started["captureId"], "token": started["token"], "expectedClipRef": recovered["clip"]["ref"]})

    def test_capture_recovery_operations_remain_advertised_while_only_slot_is_occupied(self):
        _, mapper, _, _, args = self.capture_fixture()
        preview = mapper.invoke("audio.capture.inspect", args)
        mapper.invoke("audio.capture.start", {**args, "captureId": "capture-unit-advertisement", "fence": preview["fence"], "maxDurationMs": 1000})
        status = mapper.status()
        for operation in ("audio.capture.status", "audio.capture.stop", "audio.capture.emergency-stop", "audio.capture.cleanup"):
            self.assertIn(operation, status["operations"])
        self.assertIn("audio.capture.resampling", status["capabilities"])

    def test_routing_choice_discovery_enumerates_the_parent_track(self):
        _, mapper, _, _, _ = self.capture_fixture()
        snapshot = mapper.snapshot(); track_ref = snapshot["tracks"][1]["ref"]
        discovered = mapper.discover("routing_choice", parent=track_ref)
        self.assertTrue(any(item["name"] == "Resampling" and item["direction"] == "input-type" for item in discovered["items"]))
        self.assertTrue(all(item["parentRef"] == track_ref for item in discovered["items"]))
        self.assertIn('provenance="real-live"', Path(__file__).with_name("AbletonMcpBridge").joinpath("__init__.py").read_text(encoding="utf-8"))

    def test_references_remain_stable_across_fresh_discovery(self):
        mapper = LiveObjectMapper(FakeSong())
        first = mapper.discover("track")["items"][0]["ref"]
        second = mapper.discover("track")["items"][0]["ref"]
        self.assertEqual(first, second)
        self.assertEqual(mapper.get(first)["ref"], first)

    def test_snapshot_exposes_authoritative_set_for_transport_verification(self):
        song = FakeSong()
        song.tempo = 128.0
        mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        self.assertEqual(snapshot["set"]["tempo"], 128.0)
        self.assertEqual(mapper.get(snapshot["set"]["ref"])["ref"], snapshot["set"]["ref"])

    def test_get_reports_canonical_unknown_ref_after_clip_deletion(self):
        song = FakeSong(); song.tracks[0].clip_slots[0].create_clip(4); mapper = LiveObjectMapper(song); reference = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        song.tracks[0].clip_slots[0].clip = None
        with self.assertRaisesRegex(ValueError, "unknown live ref"):
            mapper.get(reference)

    def test_main_thread_deadline_is_exclusive_at_the_expiry_millisecond(self):
        token = _DispatchToken(1000)
        with patch("ableton_mcp_remote_script.time.time", return_value=1.0):
            self.assertFalse(token.claim())
        self.assertEqual(token.state, "cancelled")

    def test_mutation_pending_count_is_released_when_submit_fails(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
        class FailingMutationQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None):
                if on_cancel is not None: raise RuntimeError("injected submit failure")
                return action()
        bridge.queue = FailingMutationQueue(); holder = {}; parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-submit-failure", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "submit-failure-key"}, holder)
        with self.assertRaisesRegex(RuntimeError, "injected submit failure"):
            bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder)
        self.assertEqual(bridge._pending_mutations, {}); self.assertEqual(bridge._executed_mutations, {}); self.assertEqual(bridge.mapper._resolve_parameter(parameter["ref"]).value, 0.5)

    def test_post_dispatch_claim_survives_its_deadline_and_releases_only_its_own_retry_count(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
        clock = [1000.0]
        class ControlledMutationQueue:
            def __init__(self): self.actions = []; self.cancellations = []
            def submit(self, action, deadline_ms=None, on_cancel=None):
                if on_cancel is None: return action()
                self.actions.append(action); self.cancellations.append(on_cancel)
                if len(self.actions) == 1:
                    clock[0] = (deadline_ms + 1) / 1000
                    raise remote_module._DispatchUncertainError("Live main-thread operation state uncertain after dispatch")
                return {"queued": True}
        bridge.queue = ControlledMutationQueue(); holder = {}; parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-post-dispatch", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        def prepare():
            preflight = bridge._dispatch_with_holder("preflight", request, holder)
            return bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "post-dispatch-key"}, holder)
        with patch("ableton_mcp_remote_script.time.time", side_effect=lambda: clock[0]):
            first = prepare(); retry = prepare(); first_deadline = int(clock[0] * 1000) + 100
            with self.assertRaisesRegex(RuntimeError, "state uncertain after dispatch"):
                bridge._dispatch_with_holder("invoke", {**request, "deadlineMs": first_deadline, "authorityToken": first["authorityToken"]}, holder)
            self.assertGreater(int(clock[0] * 1000), first_deadline)
            self.assertEqual(bridge._pending_mutations["post-dispatch-key"]["count"], 1)
            self.assertEqual(bridge._dispatch_with_holder("invoke", {**request, "deadlineMs": int(clock[0] * 1000) + 5000, "authorityToken": retry["authorityToken"]}, holder), {"queued": True})
            self.assertEqual(bridge._pending_mutations["post-dispatch-key"]["count"], 2)
            self.assertEqual(bridge.queue.actions[0]()["value"], 0.75)
            self.assertEqual(bridge._pending_mutations["post-dispatch-key"]["count"], 1)
            bridge.queue.cancellations[1](TimeoutError("retry cancelled before dispatch"))
        self.assertEqual(bridge._pending_mutations, {}); self.assertEqual(bridge.mapper._resolve_parameter(parameter["ref"]).value, 0.75)

    def test_mutation_pending_release_is_idempotent_per_queued_invocation(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
        class CaptureMutationQueue:
            def __init__(self): self.cancellations = []
            def submit(self, action, deadline_ms=None, on_cancel=None):
                if on_cancel is None: return action()
                self.cancellations.append(on_cancel); return {"queued": True}
        bridge.queue = CaptureMutationQueue(); holder = {}; parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-double-cancel", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        for _ in range(2):
            preflight = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "double-cancel-key"}, holder)
            self.assertEqual(bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder), {"queued": True})
        self.assertEqual(bridge._pending_mutations["double-cancel-key"]["count"], 2)
        cancelled = TimeoutError("injected pre-dispatch cancellation")
        bridge.queue.cancellations[0](cancelled); bridge.queue.cancellations[0](cancelled)
        self.assertEqual(bridge._pending_mutations["double-cancel-key"]["count"], 1)
        bridge.queue.cancellations[1](cancelled)
        self.assertEqual(bridge._pending_mutations, {}); self.assertEqual(bridge._executed_mutations, {}); self.assertEqual(bridge.mapper._resolve_parameter(parameter["ref"]).value, 0.5)

    def test_retirement_is_a_live_thread_barrier_for_earlier_mutations(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.queue = _MainThreadQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); events = []
        def applied():
            events.append("applied"); bridge._executed_mutations["apply-key"] = {"transactionId": "transaction-barrier", "operation": "clip.create", "argsDigest": "digest", "result": {"created": True}}
        self.assertTrue(bridge.queue.submit_nowait(applied, int(time.time() * 1000) + 5000))
        outcome = []
        worker = threading.Thread(target=lambda: outcome.append(bridge._dispatch_with_holder("retire", {"transactionId": "transaction-barrier", "deadlineMs": int(time.time() * 1000) + 5000}, {})))
        worker.start()
        deadline = time.time() + 1
        while bridge.queue.items.qsize() < 2 and time.time() < deadline: time.sleep(0.001)
        self.assertEqual(bridge.queue.items.qsize(), 2); bridge.queue.drain(); worker.join(1); bridge.queue.close()
        self.assertFalse(worker.is_alive()); self.assertEqual(events, ["applied"]); self.assertEqual(outcome, [{"retired": 1}]); self.assertEqual(bridge._executed_mutations, {})

    def test_retirement_fences_prior_key_but_allows_same_transaction_undo_key(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._executed_lock = threading.Lock()
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); holder = {}; transaction_id = "transaction-apply-undo"
        def set_value(value, key):
            parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]; request = {"operation": "device.parameter.set", "transactionId": transaction_id, "args": {"ref": parameter["ref"], "value": value, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
            preflight = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": key}, holder)
            return bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"], "transactionId": transaction_id}, holder)
        self.assertEqual(set_value(0.75, "apply-key")["value"], 0.75)
        self.assertEqual(bridge._dispatch_with_holder("retire", {"transactionId": transaction_id, "deadlineMs": int(time.time() * 1000) + 5000}, {}), {"retired": 1})
        self.assertEqual(set_value(0.5, "undo-key")["value"], 0.5)

    def test_terminal_retirement_atomically_requires_safety_and_fences_prepared_authority(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        class SafeRealtime:
            def stats(self): return {"armed": False, "pending": 0}
        bridge.queue = ImmediateQueue(); bridge._realtime = SafeRealtime(); holder = {}; transaction_id = "transaction-terminal-finalize"; parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": transaction_id, "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "terminal-prepared-key"}, holder)
        second_preflight = bridge._dispatch_with_holder("preflight", request, holder); second_prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": second_preflight["preflightToken"], "confirmation": second_preflight["confirmation"], "idempotencyKey": "terminal-prepared-key-two"}, holder)
        bridge.mapper.song.is_playing = True
        with self.assertRaisesRegex(ValueError, "stopped playback"):
            bridge._dispatch_with_holder("retire", {"transactionId": transaction_id, "terminal": True, "deadlineMs": int(time.time() * 1000) + 5000}, {})
        bridge.mapper.song.is_playing = False
        self.assertEqual(bridge._dispatch_with_holder("retire", {"transactionId": transaction_id, "terminal": True, "deadlineMs": int(time.time() * 1000) + 5000}, {}), {"retired": 0})
        with self.assertRaisesRegex(ValueError, "mismatched mutation authority"):
            bridge._dispatch_with_holder("invoke", {**request, "authorityToken": second_prepared["authorityToken"], "transactionId": "transaction-swapped"}, holder)
        with self.assertRaisesRegex(ValueError, "terminally finalized"):
            bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"], "transactionId": transaction_id}, holder)
        self.assertEqual(bridge.mapper._resolve_parameter(parameter["ref"]).value, 0.5)

    def test_retirement_tombstone_fences_a_mutation_delayed_before_enqueue(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._executed_lock = threading.Lock()
        class DelayedMutationQueue:
            def __init__(self): self.delay = False; self.entered = threading.Event(); self.release = threading.Event()
            def submit(self, action, deadline_ms=None, on_cancel=None):
                if self.delay and threading.current_thread().name == "delayed-mutation": self.entered.set(); self.release.wait(1)
                return action()
        bridge.queue = DelayedMutationQueue(); holder = {}; parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-race", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "delayed-apply"}, holder)
        bridge.queue.delay = True; failures = []
        def mutate():
            try: bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"], "transactionId": "transaction-race"}, holder)
            except Exception as error: failures.append(str(error))
        worker = threading.Thread(target=mutate, name="delayed-mutation"); worker.start(); self.assertTrue(bridge.queue.entered.wait(1))
        conflicting = {"operation": "device.parameter.set", "transactionId": "transaction-race", "args": {**request["args"], "value": 0.6}}
        conflict_preflight = bridge._dispatch_with_holder("preflight", conflicting, holder); conflict_prepared = bridge._dispatch_with_holder("prepare", {**conflicting, "preflightToken": conflict_preflight["preflightToken"], "confirmation": conflict_preflight["confirmation"], "idempotencyKey": "delayed-apply"}, holder)
        with self.assertRaisesRegex(ValueError, "idempotency key conflicts with a pending mutation"):
            bridge._dispatch_with_holder("invoke", {**conflicting, "authorityToken": conflict_prepared["authorityToken"], "transactionId": "transaction-race"}, holder)
        duplicate_preflight = bridge._dispatch_with_holder("preflight", request, holder); duplicate_prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": duplicate_preflight["preflightToken"], "confirmation": duplicate_preflight["confirmation"], "idempotencyKey": "delayed-apply"}, holder)
        bridge.mapper._resolve_parameter(parameter["ref"]).value = 0.6
        with self.assertRaisesRegex(ValueError, "Live state changed"):
            bridge._dispatch_with_holder("invoke", {**request, "authorityToken": duplicate_prepared["authorityToken"], "transactionId": "transaction-race"}, holder)
        bridge.mapper._resolve_parameter(parameter["ref"]).value = 0.5
        self.assertEqual(bridge._pending_mutations["delayed-apply"]["count"], 1)
        self.assertEqual(bridge._dispatch_with_holder("retire", {"transactionId": "transaction-race", "deadlineMs": int(time.time() * 1000) + 5000}, {}), {"retired": 0})
        bridge.queue.release.set(); worker.join(1)
        self.assertFalse(worker.is_alive()); self.assertEqual(failures, ["mutation replay authority has been retired; nothing changed"]); self.assertEqual(bridge.mapper._resolve_parameter(parameter["ref"]).value, 0.5); self.assertEqual(bridge._executed_mutations, {})

    def test_mutation_preflight_is_unpredictable_one_use_and_fences_external_state(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong())
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-preflight", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **self.parameter_authority(bridge.mapper, parameter["ref"])}}
        first = bridge._dispatch_with_holder("preflight", request, holder)
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": first["preflightToken"], "confirmation": "x" * 24, "idempotencyKey": "wrong-confirmation"}, holder)
        second = bridge._dispatch_with_holder("preflight", request, holder); bridge.mapper._resolve_parameter(parameter["ref"]).value = 0.6
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": second["preflightToken"], "confirmation": second["confirmation"], "idempotencyKey": "external-edit"}, holder)
        bridge.mapper._resolve_parameter(parameter["ref"]).value = 0.5
        third = bridge._dispatch_with_holder("preflight", request, holder); prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": third["preflightToken"], "confirmation": third["confirmation"], "idempotencyKey": "confirmed-apply"}, holder)
        result = bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder)
        self.assertTrue(result["changed"])
        second_connection = {}
        replay_preflight = bridge._dispatch_with_holder("preflight", request, second_connection); replay_prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": replay_preflight["preflightToken"], "confirmation": replay_preflight["confirmation"], "idempotencyKey": "confirmed-apply"}, second_connection)
        self.assertEqual(bridge._dispatch_with_holder("invoke", {**request, "authorityToken": replay_prepared["authorityToken"]}, second_connection), result)
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("invoke", {**request, "authorityToken": replay_prepared["authorityToken"]}, second_connection)
        swapped = {**request, "transactionId": "transaction-replay-swap"}; swapped_preflight = bridge._dispatch_with_holder("preflight", swapped, second_connection); swapped_prepared = bridge._dispatch_with_holder("prepare", {**swapped, "preflightToken": swapped_preflight["preflightToken"], "confirmation": swapped_preflight["confirmation"], "idempotencyKey": "confirmed-apply"}, second_connection)
        with self.assertRaisesRegex(ValueError, "conflicts with an executed mutation"):
            bridge._dispatch_with_holder("invoke", {**swapped, "authorityToken": swapped_prepared["authorityToken"]}, second_connection)
        bridge.mapper.song.cue_points = []; bridge.mapper.song.set_or_delete_cue = lambda: None
        locator_request = {"operation": "locator.add", "transactionId": "transaction-locator", "args": {"name": "Prepared", "position": 8.0}}
        locator_preflight = bridge._dispatch_with_holder("preflight", locator_request, holder); bridge.mapper.song.cue_points.append(FakeLocator(4.0, "External"))
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**locator_request, "preflightToken": locator_preflight["preflightToken"], "confirmation": locator_preflight["confirmation"], "idempotencyKey": "locator-external-edit"}, holder)
        bridge._realtime_op = lambda operation, args: {"armed": operation == "realtime.arm"}
        realtime_request = {"operation": "realtime.arm", "transactionId": "transaction-realtime", "args": {"ttlMs": 5000, "channels": ["udp-json"], "parameterRefs": [], "targetAuthorities": [], "outputSafety": {"safe": True, "provenance": "unit-test"}}}
        realtime_preflight = bridge._dispatch_with_holder("preflight", realtime_request, holder); realtime_prepared = bridge._dispatch_with_holder("prepare", {**realtime_request, "preflightToken": realtime_preflight["preflightToken"], "confirmation": realtime_preflight["confirmation"], "idempotencyKey": "realtime-state-fence"}, holder)
        bridge.mapper.song.tempo = 130.0
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("invoke", {**realtime_request, "authorityToken": realtime_prepared["authorityToken"]}, holder)

    def test_mutation_authority_excludes_only_drifting_transport_position(self):
        song = FakeSong(); song.is_playing = True; song.current_song_time = 1.0
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song)
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        request = {"operation": "locator.add", "transactionId": "transaction-position", "args": {"name": "Position Fence", "position": 8.0}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder); song.current_song_time = 3.5
        prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "locator-position-drift"}, holder)
        self.assertEqual(prepared["operation"], "locator.add")
        changed = bridge._dispatch_with_holder("preflight", request, holder); song.is_playing = False
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": changed["preflightToken"], "confirmation": changed["confirmation"], "idempotencyKey": "locator-playback-change"}, holder)

    def test_mutations_and_reads_go_through_while_live_plays(self):
        # While a scene plays, Live moves the playhead (the Set row's position) and each playing clip's
        # playing_position every display tick, and playing automation moves its parameter. Preflight,
        # prepare and invoke land on different ticks, so none of that may be part of their fence.
        song = FakeSong(); song.is_playing = True; song.current_song_time = 1272.0; song.tempo = 120.0
        clip = FakeClip(4.0); clip.name = "Loop"; clip.playing_position = 0.25; clip.is_playing = True; song.tracks[0].clip_slots[0].clip = clip
        automated = song.tracks[0].devices[0].parameters[0]; automated.automation_state = "playing"
        stops = []; song.stop_playing = lambda: stops.append(True)
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song)
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        snapshot = bridge.mapper.snapshot(); set_row = snapshot["set"]; track_row = snapshot["tracks"][0]; clip_row = track_row["clips"][0]; parameter_row = track_row["devices"][0]["parameters"][0]
        def play_on():
            song.current_song_time += 0.37; clip.playing_position = (clip.playing_position + 0.37) % 4.0; automated.value = (automated.value + 0.1) % 1.0
        def authorize(request, key):
            preflight = bridge._dispatch_with_holder("preflight", request, holder); play_on()
            prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": key}, holder); play_on()
            return prepared
        stop = {"operation": "transport.action", "transactionId": "transaction-stop-while-playing", "args": {"setRef": set_row["ref"], "action": "stop", "expectedObjectIdentity": set_row["objectIdentity"], "expectedRevision": str(bridge.mapper._playback()["revision"])}}
        prepared = authorize(stop, "stop-while-playing")
        self.assertEqual(bridge._dispatch_with_holder("invoke", {**stop, "authorityToken": prepared["authorityToken"]}, holder)["done"], True); self.assertEqual(stops, [True])
        # A clip edit names the playing clip, a track edit its track, a parameter edit the automated parameter.
        for operation, args, key in (("clip.set", {"ref": clip_row["ref"], "name": "Edited"}, "clip-edit-while-playing"), ("track.set", {"ref": track_row["ref"], "colorIndex": 3}, "track-edit-while-playing"), ("device.parameter.set", {"ref": parameter_row["ref"], "value": 0.5}, "automated-parameter-while-playing")):
            self.assertEqual(authorize({"operation": operation, "transactionId": f"transaction-{key}", "args": args}, key)["operation"], operation)
        # Reading the song needs no mutation authority at all.
        song.signature_numerator = 4; song.signature_denominator = 4; song.swing_amount = 0.0
        state = bridge._dispatch_with_holder("invoke", {"operation": "song.read", "args": {"setRef": set_row["ref"]}}, holder); play_on()
        self.assertEqual(state["signatureNumerator"], 4)
        # A real, discrete change between preflight and prepare still refuses.
        edit = {"operation": "clip.set", "transactionId": "transaction-clip-edit-refused", "args": {"ref": clip_row["ref"], "name": "Edited"}}
        preflight = bridge._dispatch_with_holder("preflight", edit, holder); clip.name = "Renamed by hand"
        with self.assertRaisesRegex(ValueError, "mismatched mutation preflight"): bridge._dispatch_with_holder("prepare", {**edit, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "clip-edit-refused"}, holder)
        manual = {"operation": "device.parameter.set", "transactionId": "transaction-parameter-refused", "args": {"ref": parameter_row["ref"], "value": 0.5}}
        automated.automation_state = "none"; preflight = bridge._dispatch_with_holder("preflight", manual, holder); automated.value = 0.9
        with self.assertRaisesRegex(ValueError, "mismatched mutation preflight"): bridge._dispatch_with_holder("prepare", {**manual, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "parameter-refused"}, holder)

    def test_playing_mixer_and_tempo_automation_leave_the_authority_fence_alone(self):
        # Volume, send and tempo automation move those values every tick while Live plays. The Set's and
        # the track's rows carry the values but not their automation state, which the parameters are asked.
        song = FakeSong(); song.is_playing = True; song.tempo = 120.0
        track = song.tracks[0]; track.mixer_device = FakeMixerDevice(); mixer = track.mixer_device
        master = FakeTrack(); master.mixer_device = FakeMixerDevice(); song.master_track = master; song_tempo = master.mixer_device.song_tempo
        for parameter in (mixer.volume, mixer.sends[1], song_tempo): parameter.automation_state = "playing"
        stops = []; song.stop_playing = lambda: stops.append(True)
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song)
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        snapshot = bridge.mapper.snapshot(); set_row = snapshot["set"]; track_row = snapshot["tracks"][0]
        self.assertIsNotNone(track_row["mixer"]["volumeRef"]); self.assertEqual(len(track_row["mixer"]["sendRefs"]), 2)
        def play_on():
            song.current_song_time += 0.37; mixer.volume.value = (mixer.volume.value + 0.05) % 1.0; mixer.sends[1].value = (mixer.sends[1].value + 0.07) % 1.0
            song.tempo += 0.5; song_tempo.value = song.tempo
        def authorize(request, key):
            preflight = bridge._dispatch_with_holder("preflight", request, holder); play_on()
            prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": key}, holder); play_on()
            return prepared
        track.color_index = 0; fences = {"expectedObjectIdentity": track_row["objectIdentity"], "expectedStateRevision": hashlib.sha256(bridge.mapper._bounded_canonical(bridge.mapper._track_properties_state(track)).encode("utf-8")).hexdigest()}
        edit = {"operation": "track.set", "transactionId": "transaction-track-edit-under-automation", "args": {"ref": track_row["ref"], "colorIndex": 3, **fences}}
        prepared = authorize(edit, "track-edit-under-automation")
        bridge._dispatch_with_holder("invoke", {**edit, "authorityToken": prepared["authorityToken"]}, holder); self.assertEqual(track.color_index, 3)
        stop = {"operation": "transport.action", "transactionId": "transaction-stop-under-tempo-automation", "args": {"setRef": set_row["ref"], "action": "stop", "expectedObjectIdentity": set_row["objectIdentity"], "expectedRevision": str(bridge.mapper._playback()["revision"])}}
        prepared = authorize(stop, "stop-under-tempo-automation")
        self.assertEqual(bridge._dispatch_with_holder("invoke", {**stop, "authorityToken": prepared["authorityToken"]}, holder)["done"], True); self.assertEqual(stops, [True])
        # The same values moved by hand, with no automation playing them, are real changes and still refuse.
        for key, change in (("pan-by-hand", lambda: setattr(mixer.panning, "value", 0.9)), ("tempo-by-hand", lambda: setattr(song, "tempo", 90.0))):
            if key == "tempo-by-hand": song_tempo.automation_state = "none"
            request = {"operation": "track.set", "transactionId": f"transaction-{key}", "args": {"ref": track_row["ref"], "colorIndex": 4}}
            preflight = bridge._dispatch_with_holder("preflight", request, holder); change()
            with self.assertRaisesRegex(ValueError, "mismatched mutation preflight"): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": key}, holder)

    def test_capture_stop_authority_survives_watchdog_stop_but_not_identity_change(self):
        song = FakeSong(); song.is_playing = True
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song)
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        bridge.mapper._capture_state = {"captureId": "capture-test", "startedAt": 1000, "state": "active", "sourceSlotRef": "source-slot", "destinationSlotRef": "destination-slot", "destinationTrackRef": "destination-track"}
        request = {"operation": "audio.capture.stop", "transactionId": "transaction-capture-stop", "args": {"captureId": "capture-test", "token": "t" * 24}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder)
        bridge.mapper._capture_state["state"] = "stopped"; song.is_playing = False
        prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "capture-watchdog-stop"}, holder)
        self.assertEqual(prepared["operation"], "audio.capture.stop")
        changed = bridge._dispatch_with_holder("preflight", request, holder); bridge.mapper._capture_state["captureId"] = "replacement-capture"
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": changed["preflightToken"], "confirmation": changed["confirmation"], "idempotencyKey": "capture-identity-change"}, holder)

    def test_capture_cleanup_authority_ignores_native_media_finalization_drift(self):
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = FakeClip(4.0)
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song)
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        clip = bridge.mapper.snapshot()["tracks"][0]["clips"][0]; owned_clip = song.tracks[0].clip_slots[0].clip
        bridge.mapper._capture_state = {"captureId": "capture-test", "state": "captured", "sourceSlotRef": "source-slot", "destinationSlotRef": "destination-slot", "clipRef": clip["ref"], "_destinationSlot": song.tracks[0].clip_slots[0], "_ownedClip": owned_clip, "_ownedClipIdentity": bridge.mapper._capture_object_identity(owned_clip), "residual": []}
        request = {"operation": "audio.capture.cleanup", "transactionId": "transaction-capture-cleanup", "args": {"captureId": "capture-test", "token": "t" * 24, "expectedClipRef": clip["ref"]}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder)
        owned_clip.length = 4.25; song.tempo = 127.0
        prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "capture-cleanup-finalization"}, holder)
        self.assertEqual(prepared["operation"], "audio.capture.cleanup")
        replacement_preflight = bridge._dispatch_with_holder("preflight", request, holder)
        song.tracks[0].clip_slots[0].clip = FakeClip(4.25)
        with self.assertRaises(ValueError): bridge._dispatch_with_holder("prepare", {**request, "preflightToken": replacement_preflight["preflightToken"], "confirmation": replacement_preflight["confirmation"], "idempotencyKey": "capture-cleanup-replacement"}, holder)

    def test_scene_capture_claims_the_identity_distinct_scene_with_duplicate_names(self):
        song = FakeSong(); song.scenes = [FakeScene("Duplicate"), FakeScene("Duplicate")]; created = FakeScene("Duplicate")
        song.capture_and_insert_scene = lambda: song.scenes.insert(1, created)
        mapper = LiveObjectMapper(song); result = mapper.invoke("scene.capture", {"expectedStateRevision": mapper._capture_authority_revision()})
        self.assertIs(mapper.refs.get(result["ref"]), created); self.assertEqual(result["objectIdentity"], mapper._capture_object_identity(created))

    def test_scene_capture_authority_refuses_truncated_warp_markers(self):
        class Marker:
            def __init__(self, value): self.beat_time = value; self.sample_time = value * 100.0
        song = FakeSong(); clip = FakeClip(4.0); clip.warp_markers = [Marker(float(index)) for index in range(257)]; song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song)
        # A clip's markers aren't capped: 257 read whole. Only past the discovery bound is a read refused.
        mapper._capture_authority_revision()
        with patch.object(remote_module, "MAX_DISCOVERY_COLLECTION_LENGTH", 256), self.assertRaisesRegex(ValueError, "warp-marker content exceeds"):
            mapper._capture_authority_revision()

    def test_scene_capture_authority_refuses_unreadable_warp_markers(self):
        class UnreadableWarpClip(FakeClip):
            @property
            def warp_markers(self): raise RuntimeError("unreadable")
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = UnreadableWarpClip(4.0); mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(ValueError, "warp-marker collection is unreadable"):
            mapper._capture_authority_revision()

    def test_owned_delete_refuses_replacements_at_the_same_traversal_location(self):
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = FakeClip(4.0); mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); clip_ref = snapshot["tracks"][0]["clips"][0]["ref"]; original_clip = song.tracks[0].clip_slots[0].clip; clip_authority = mapper._session_clip_authority(clip_ref)
        song.tracks[0].clip_slots[0].clip = FakeClip(4.0)
        with self.assertRaises(ValueError): mapper.invoke("clip.delete", {"ref": clip_ref, **clip_authority})
        scene_ref = snapshot["scenes"][0]["ref"]; scene_identity = mapper._capture_object_identity(song.scenes[0]); song.scenes[0] = FakeScene("Replacement")
        with self.assertRaises(ValueError): mapper.invoke("scene.delete", {"ref": scene_ref, "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": scene_identity})
        self.assertEqual(song.scenes[0].name, "Replacement")

    def test_subscription_rejects_event_types_without_a_producer(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong()); holder = {}
        for event_type in ("state", "meter", "max", "osc", "transport", "object"):
            with self.assertRaisesRegex(ValueError, "subscription types are invalid or unavailable"):
                bridge._subscribe_main({"args": {"types": [event_type]}}, holder)

        class ListenerSong(FakeSong):
            def __init__(self): super().__init__(); self.listeners = {name: [] for name in ("is_playing", "record_mode", "session_record", "tracks", "scenes")}
            def _add(self, name, callback): self.listeners[name].append(callback)
            def _remove(self, name, callback): self.listeners[name].remove(callback)
            def add_is_playing_listener(self, callback): self._add("is_playing", callback)
            def remove_is_playing_listener(self, callback): self._remove("is_playing", callback)
            def add_record_mode_listener(self, callback): self._add("record_mode", callback)
            def remove_record_mode_listener(self, callback): self._remove("record_mode", callback)
            def add_session_record_listener(self, callback): self._add("session_record", callback)
            def remove_session_record_listener(self, callback): self._remove("session_record", callback)
            def add_tracks_listener(self, callback): self._add("tracks", callback)
            def remove_tracks_listener(self, callback): self._remove("tracks", callback)
            def add_scenes_listener(self, callback): self._add("scenes", callback)
            def remove_scenes_listener(self, callback): self._remove("scenes", callback)

        bridge.mapper = LiveObjectMapper(ListenerSong())
        result = bridge._subscribe_main({"args": {"types": ["transport", "object"]}}, holder)
        self.assertTrue(result["subscribed"]); holder["subscription"].close()

    def test_subscription_mid_registration_failure_leaves_no_registered_callbacks(self):
        class PartialListenerSong(FakeSong):
            def __init__(self): super().__init__(); self.listeners = {name: [] for name in ("is_playing", "record_mode", "session_record", "tracks", "scenes")}
            def _add(self, name, callback): self.listeners[name].append(callback)
            def _remove(self, name, callback): self.listeners[name].remove(callback)
            def add_is_playing_listener(self, callback): self._add("is_playing", callback)
            def remove_is_playing_listener(self, callback): self._remove("is_playing", callback)
            def add_record_mode_listener(self, callback): self._add("record_mode", callback)
            def remove_record_mode_listener(self, callback): self._remove("record_mode", callback)
            def add_session_record_listener(self, callback): self._add("session_record", callback)
            def remove_session_record_listener(self, callback): self._remove("session_record", callback)
            def add_tracks_listener(self, callback): self._add("tracks", callback)
            def remove_tracks_listener(self, callback): self._remove("tracks", callback)
            def add_scenes_listener(self, callback): raise RuntimeError("listener registration rejected")
            def remove_scenes_listener(self, callback): self._remove("scenes", callback)

        song = PartialListenerSong(); mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(RuntimeError, "listener registration rejected"):
            _Subscription(mapper, {"transport", "object"})
        self.assertTrue(all(callbacks == [] for callbacks in song.listeners.values()))

    def test_subscription_coalescing_preserves_continuity_without_false_overflow(self):
        mapper = LiveObjectMapper(FakeSong()); subscription = _Subscription(mapper, {"object"})
        subscription._emit("object", {"index": 1}); subscription._emit("object", {"index": 2})
        events = subscription.drain()
        self.assertEqual([event["type"] for event in events], ["reset", "object"])
        self.assertEqual([event["sequence"] for event in events], [1, 2])
        self.assertEqual(events[-1]["payload"], {"index": 2}); self.assertEqual(events[-1]["coalesced"], 1)

    def test_subscription_overflow_emits_epoch_bound_reset(self):
        mapper = LiveObjectMapper(FakeSong())
        with patch.object(remote_module, "MAX_PENDING_EVENTS", 256):
            subscription = _Subscription(mapper, {"transport", "object"})
            for index in range(300): subscription._emit("transport" if index % 2 == 0 else "object", {"index": index})
        events = subscription.drain(); reset = events[-1]
        self.assertEqual(reset["type"], "reset"); self.assertEqual(reset["epoch"], mapper.refs.epoch); self.assertTrue(reset["payload"]["resnapshot"]); self.assertGreater(reset["payload"]["overflow"], 0)
        old_epoch = mapper.refs.epoch; mapper.invoke("session.reconnect", {}); subscription._emit("object", {"afterReconnect": True}); reconnected = subscription.drain()
        self.assertNotEqual(mapper.refs.epoch, old_epoch); self.assertEqual(reconnected[0]["type"], "reset"); self.assertTrue(all(event["epoch"] == mapper.refs.epoch for event in reconnected))

    def test_browser_never_classifies_generic_loadable_clips_as_devices(self):
        class Item:
            def __init__(self, name, loadable=False, children=None): self.name = name; self.is_loadable = loadable; self.children = children or []
        class Browser:
            def __init__(self, song): self.song = song; self.instruments = Item("instruments", children=[Item("Synth", True)]); self.clips = Item("clips", children=[Item("Loop.wav", True)]); self.loaded = []
            def load_item(self, item): self.loaded.append(item); self.song.view.selected_track.devices.append(FakeDevice())
        song = FakeSong(); song.tracks.append(FakeTrack()); song.tracks[0].devices = []; song.view = type("View", (), {"selected_track": song.tracks[1]})(); browser = Browser(song); mapper = LiveObjectMapper(song); mapper._browser = lambda: browser
        instrument = mapper.invoke("browser.search", {"category": "instruments", "limit": 10})["items"][0]; clip = mapper.invoke("browser.search", {"category": "clips", "limit": 10})["items"][0]
        self.assertTrue(instrument["isDevice"]); self.assertFalse(clip["isDevice"])
        with self.assertRaises(ValueError): mapper.invoke("browser.load", {"itemId": clip["id"], "trackRef": mapper.snapshot()["tracks"][0]["ref"], "expectedName": clip["name"]})
        self.assertEqual(browser.loaded, [])
        with self.assertRaises(ValueError): mapper.invoke("browser.load", {"itemId": instrument["id"], "expectedName": instrument["name"]})
        track_row = mapper.snapshot()["tracks"][0]; track_ref = track_row["ref"]; track_authority = {"expectedTrackIdentity": track_row["objectIdentity"], "expectedSiblings": [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in track_row["devices"]]}
        result = mapper.invoke("browser.load", {"itemId": instrument["id"], "trackRef": track_ref, "expectedName": instrument["name"], "expectedItemIdentity": instrument["objectIdentity"], **track_authority})
        self.assertTrue(result["loaded"]); self.assertIs(song.view.selected_track, song.tracks[1]); self.assertEqual(len(song.tracks[0].devices), 1)
        song.return_tracks = [FakeTrack()]; return_ref = next(row["ref"] for row in mapper.snapshot()["tracks"] if row["kind"] == "return")
        with self.assertRaises(ValueError): mapper.invoke("browser.load", {"itemId": instrument["id"], "trackRef": return_ref, "expectedName": instrument["name"]})
        self.assertEqual(len(browser.loaded), 1)

    def test_browser_failure_cleans_transaction_owned_device_before_returning(self):
        class Item:
            def __init__(self, name, loadable=False, children=None): self.name = name; self.is_loadable = loadable; self.children = children or []
        class Browser:
            def __init__(self, song): self.song = song; self.fail = True; self.instruments = Item("instruments", children=[Item("Failing Synth", True)])
            def load_item(self, _item):
                self.song.view.selected_track.devices.insert(0, FakeDevice())
                if self.fail: raise RuntimeError("injected browser failure")
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.delete_device = lambda index: track.devices.pop(index); song.view = type("View", (), {"selected_track": track})(); mapper = LiveObjectMapper(song); browser = Browser(song); mapper._browser = lambda: browser; item = mapper.invoke("browser.search", {"category": "instruments", "limit": 10})["items"][0]; row = mapper.snapshot()["tracks"][0]; authority = {"expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in row["devices"]]}
        with self.assertRaisesRegex(ValueError, "without a residual device"): mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], **authority})
        self.assertEqual(len(track.devices), 0)
        browser.fail = False; row = mapper.snapshot()["tracks"][0]; authority = {"expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": []}; registry_before = mapper.refs.checkpoint(); mapper._mapped_fingerprint = lambda _reference: (_ for _ in ()).throw(RuntimeError("injected fingerprint failure"))
        with self.assertRaisesRegex(ValueError, "mapping failed without a residual device"): mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], **authority})
        self.assertEqual(len(track.devices), 0); self.assertEqual(mapper.refs.checkpoint(), registry_before)

    def test_real_live_browser_load_returns_hidden_cleanup_ownership_shape(self):
        class Item:
            def __init__(self, name, children=None): self.name = name; self.children = children or []; self.is_loadable = not bool(children); self.is_device = not bool(children)
        class Browser:
            def __init__(self, song): self.song = song; self.instruments = Item("instruments", [Item("Owned Synth")])
            def load_item(self, _item): self.song.view.selected_track.devices.append(FakeDevice())
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.delete_device = lambda index: track.devices.pop(index); song.view = type("View", (), {"selected_track": track})(); mapper = LiveObjectMapper(song, provenance="real-live"); browser = Browser(song); mapper._browser = lambda: browser; item = mapper.invoke("browser.search", {"category": "instruments", "limit": 10})["items"][0]; row = mapper.snapshot()["tracks"][0]; transaction = "browser-owned-transaction"; loaded = mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in row["devices"]]}, transaction)
        self.assertIn("ownershipToken", loaded); snapshot = mapper.snapshot(); track_row = snapshot["tracks"][0]; device = next(item for item in track_row["devices"] if item["ref"] == loaded["deviceRef"]); siblings = [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in track_row["devices"]]; deleted = mapper.invoke("device.delete", {"ref": device["ref"], "expectedObjectIdentity": device["objectIdentity"], "expectedOwnerRef": track_row["ref"], "expectedOwnerIdentity": track_row["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track_row["ref"], "expectedTrackIdentity": track_row["objectIdentity"]}, transaction, loaded["ownershipToken"]); self.assertEqual(deleted, {"deleted": device["ref"]}); self.assertEqual(len(track.devices), 0)

    def test_a_device_that_settles_after_loading_records_its_settled_state_and_undoes(self):
        class Item:
            def __init__(self, name, children=None): self.name = name; self.children = children or []; self.is_loadable = not bool(children); self.is_device = not bool(children)
        class Browser:
            def __init__(self, song): self.song = song; self.audio_effects = Item("audio_effects", [Item("LFO")])
            def load_item(self, _item): device = FakeDevice(); device.name = "LFO"; self.song.view.selected_track.devices.append(device)
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.delete_device = lambda index: track.devices.pop(index); song.view = type("View", (), {"selected_track": track})(); mapper = LiveObjectMapper(song, provenance="real-live"); browser = Browser(song); mapper._browser = lambda: browser
        item = mapper.invoke("browser.search", {"category": "audio_effects", "limit": 10})["items"][0]; row = mapper.snapshot()["tracks"][0]; transaction = "settling-transaction"
        loaded = mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": []}, transaction)
        # Max for Live builds the device after the load returns: its parameters arrive late.
        track.devices[0].parameters.append(FakeParameter()); settled = mapper._ownership_fingerprint(loaded["deviceRef"])
        self.assertNotEqual(settled, loaded["createdFingerprint"])
        settle = {"ref": loaded["deviceRef"], "expectedObjectIdentity": loaded["deviceObjectIdentity"], "expectedFingerprint": settled}
        with self.assertRaisesRegex(ValueError, "creating transaction"): mapper.invoke("ownership.settle", settle, "another-transaction", loaded["ownershipToken"])
        with self.assertRaisesRegex(ValueError, "still changing"): mapper.invoke("ownership.settle", {**settle, "expectedFingerprint": "0" * 64}, transaction, loaded["ownershipToken"])
        self.assertEqual(mapper.invoke("ownership.settle", settle, transaction, loaded["ownershipToken"]), {"settled": True, "fingerprint": settled})
        with self.assertRaisesRegex(ValueError, "just made"): mapper.invoke("ownership.settle", settle, transaction, loaded["ownershipToken"])
        track_row = mapper.snapshot()["tracks"][0]; device = track_row["devices"][0]
        deleted = mapper.invoke("device.delete", {"ref": device["ref"], "expectedObjectIdentity": device["objectIdentity"], "expectedOwnerRef": track_row["ref"], "expectedOwnerIdentity": track_row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]}], "expectedTrackRef": track_row["ref"], "expectedTrackIdentity": track_row["objectIdentity"]}, transaction, loaded["ownershipToken"])
        self.assertEqual(deleted, {"deleted": device["ref"]}); self.assertEqual(track.devices, [])

    def test_automation_batch_failure_restores_exact_prior_envelope(self):
        class Event:
            def __init__(self, time, value): self.time = time; self.value = value
        class Envelope:
            def __init__(self, clip): self.canonical_parent = clip; self.events = [Event(0.25, 0.2)]; self.fail_delete = False; self.mutate_other = False
            def events_in_range(self, start, end): return [event for event in self.events if start <= event.time < end]
            def create_event(self, event):
                self.events.append(event)
                if self.mutate_other and self.events: self.events[0].value = 0.9
                if event.value == 0.75: raise RuntimeError("injected event failure")
            def delete_events_in_range(self, start, end):
                if self.fail_delete:
                    for index, event in enumerate(self.events):
                        if start <= event.time < end: self.events.pop(index); break
                    raise RuntimeError("injected partial delete failure")
                self.events = [event for event in self.events if not start <= event.time < end]
        class AutomationClip(FakeClip):
            def __init__(self): super().__init__(4.0); self.envelope = Envelope(self)
            def automation_envelope(self, _parameter): return self.envelope
            def create_automation_envelope(self, _parameter): self.envelope = Envelope(self); self.envelope.events = []; return self.envelope
            def clear_envelope(self, _parameter): self.envelope = None
        song = FakeSong(); clip = AutomationClip(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); clip_ref = snapshot["tracks"][0]["clips"][0]["ref"]; parameter_ref = snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]; read = mapper.invoke("automation.envelope.read", {"clipRef": clip_ref, "parameterRef": parameter_ref}); authority = mapper._envelope_authority_digest(clip_ref, parameter_ref)
        with self.assertRaisesRegex(RuntimeError, "injected event failure"): mapper.invoke("automation.point.insert", {"clipRef": clip_ref, "parameterRef": parameter_ref, "expectedAuthorityDigest": authority, "expectedEnvelopeRevision": read["revision"], "points": [{"time": 0.5, "value": 0.5}, {"time": 0.75, "value": 0.75}]})
        self.assertEqual([(event.time, event.value) for event in clip.envelope.events], [(0.25, 0.2)])
        clip.envelope.mutate_other = True; read = mapper.invoke("automation.envelope.read", {"clipRef": clip_ref, "parameterRef": parameter_ref}); authority = mapper._envelope_authority_digest(clip_ref, parameter_ref)
        with self.assertRaisesRegex(ValueError, "exact requested state"): mapper.invoke("automation.point.insert", {"clipRef": clip_ref, "parameterRef": parameter_ref, "expectedAuthorityDigest": authority, "expectedEnvelopeRevision": read["revision"], "points": [{"time": 0.5, "value": 0.6}]})
        self.assertEqual([(event.time, event.value) for event in clip.envelope.events], [(0.25, 0.2)])
        clip.envelope.events.append(Event(0.5, 0.4)); clip.envelope.fail_delete = True; read = mapper.invoke("automation.envelope.read", {"clipRef": clip_ref, "parameterRef": parameter_ref}); authority = mapper._envelope_authority_digest(clip_ref, parameter_ref)
        with self.assertRaisesRegex(RuntimeError, "partial delete failure"): mapper.invoke("automation.point.delete", {"clipRef": clip_ref, "parameterRef": parameter_ref, "expectedAuthorityDigest": authority, "expectedEnvelopeRevision": read["revision"], "from": 0.2, "to": 0.6})
        self.assertEqual([(event.time, event.value) for event in clip.envelope.events], [(0.25, 0.2), (0.5, 0.4)])
        clip.create_automation_envelope = None; read = mapper.invoke("automation.envelope.read", {"clipRef": clip_ref, "parameterRef": parameter_ref}); authority = mapper._envelope_authority_digest(clip_ref, parameter_ref)
        with self.assertRaisesRegex(ValueError, "restoration"): mapper.invoke("automation.envelope.delete", {"clipRef": clip_ref, "parameterRef": parameter_ref, "expectedAuthorityDigest": authority, "expectedEnvelopeRevision": read["revision"]})
        self.assertIsNotNone(clip.envelope)

    def test_midi_snapshot_does_not_read_audio_only_warp_markers(self):
        class StrictMidiClip(FakeClip):
            is_audio_clip = False
            @property
            def warp_markers(self): raise RuntimeError("Warp markers are only available for Audio Clips")
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = StrictMidiClip(4.0)
        row = LiveObjectMapper(song).snapshot()["tracks"][0]["clips"][0]
        self.assertFalse(row["isAudio"]); self.assertEqual(row["availableAudioFields"], []); self.assertEqual(row["warpMarkers"], [])

    def test_audio_fields_are_discovered_and_mutated_only_when_writable(self):
        song = FakeSong(); clip = FakeCapturedAudioClip(); clip.is_recording = False; clip.pitch_coarse = 0.0; clip.pitch_fine = 0.0; clip.loop_start = 0.0; clip.loop_end = 2.0; clip.warp_mode = 1; clip.warping = True; clip.fade_in_length = 0.0; clip.fade_out_length = 0.0
        song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertIn("fadeInLength", row["availableAudioFields"]); self.assertEqual(row["warpMarkers"], [])
        fields = ("gain", "pitchCoarse", "pitchFine", "loopStart", "loopEnd", "warpMode", "warping", "fadeInLength", "fadeOutLength")
        authority = hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(); state = hashlib.sha256(mapper._bounded_canonical({field: row.get(field) for field in fields}).encode()).hexdigest()
        result = mapper.invoke("audio.clip.set", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": authority, "expectedStateRevision": state, "warping": False, "fadeInLength": 0.25, "fadeOutLength": 0.5})
        self.assertTrue(result["changed"]); self.assertFalse(clip.warping); self.assertEqual(clip.fade_out_length, 0.5)
        del clip.fade_in_length; row = mapper.get(row["ref"]); state = hashlib.sha256(mapper._bounded_canonical({field: row.get(field) for field in fields}).encode()).hexdigest()
        with self.assertRaises(ValueError): mapper.invoke("audio.clip.set", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": authority, "expectedStateRevision": state, "fadeInLength": 0.1})

    def test_audio_clip_pitch_is_written_as_the_whole_number_live_takes(self):
        class TypedAudioClip(FakeCapturedAudioClip):
            """Like Live: pitch_coarse is an int property whose setter refuses a float (Boost ArgumentError)."""
            @property
            def pitch_coarse(self): return self.__dict__.get("_pitch_coarse", 0)
            @pitch_coarse.setter
            def pitch_coarse(self, value):
                if not isinstance(value, int) or isinstance(value, bool): raise ArgumentError("Python argument types in None.None(AudioClip, float) did not match C++ signature")
                self.__dict__["_pitch_coarse"] = value
            @property
            def gain(self): return self.__dict__.get("_gain", 1.0)
            @gain.setter
            def gain(self, value): self.__dict__["_gain"] = float32(value)
        song = FakeSong(); clip = TypedAudioClip(); clip.is_recording = False; clip.pitch_fine = 0.0; clip.loop_start = 0.0; clip.loop_end = 2.0; clip.warp_mode = 1; clip.warping = True; clip.fade_in_length = 0.0; clip.fade_out_length = 0.0
        song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        fields = ("gain", "pitchCoarse", "pitchFine", "loopStart", "loopEnd", "warpMode", "warping", "fadeInLength", "fadeOutLength")
        def request(**edits):
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(), "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest(), **edits}
        validate_operation_payload("audio.clip.set", "request", request(pitchCoarse=-12, gain=0.6))
        self.assertTrue(mapper.invoke("audio.clip.set", request(pitchCoarse=-12, gain=0.6))["changed"])
        self.assertIs(type(clip.pitch_coarse), int); self.assertEqual(clip.pitch_coarse, -12); self.assertAlmostEqual(clip.gain, 0.6, places=6)
        with self.assertRaisesRegex(ValueError, "pitchCoarse must be a whole number"): mapper.invoke("audio.clip.set", request(pitchCoarse=-11.5))
        self.assertEqual(clip.pitch_coarse, -12)

    def test_audio_multi_field_failure_rolls_back_exact_prior_state(self):
        class FailingAudioClip(FakeCapturedAudioClip):
            def __init__(self): self._fade_out = 0.0; super().__init__()
            @property
            def fade_out_length(self): return self._fade_out
            @fade_out_length.setter
            def fade_out_length(self, value):
                if value == 0.5: raise RuntimeError("injected fade failure")
                self._fade_out = value
        song = FakeSong(); clip = FailingAudioClip(); clip.is_recording = False; clip.pitch_coarse = 0.0; clip.pitch_fine = 0.0; clip.loop_start = 0.0; clip.loop_end = 2.0; clip.warp_mode = 1; clip.warping = True; clip.fade_in_length = 0.0
        song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]; fields = ("gain", "pitchCoarse", "pitchFine", "loopStart", "loopEnd", "warpMode", "warping", "fadeInLength", "fadeOutLength"); authority = hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(); state = hashlib.sha256(mapper._bounded_canonical({field: row.get(field) for field in fields}).encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, "loopStart"):
            mapper.invoke("audio.clip.set", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": authority, "expectedStateRevision": state, "gain": 0.25, "loopStart": 3.0, "loopEnd": 2.0})
        self.assertEqual(clip.gain, 1.0)
        with self.assertRaisesRegex(RuntimeError, "injected fade failure"):
            mapper.invoke("audio.clip.set", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": authority, "expectedStateRevision": state, "gain": 0.25, "fadeOutLength": 0.5})
        self.assertEqual(clip.gain, 1.0); self.assertEqual(clip.fade_out_length, 0.0)

    def test_nested_chain_devices_and_parameters_are_first_class(self):
        class EnableableDevice(FakeDevice):
            def __init__(self):
                super().__init__()
                on = FakeParameter(); on.value = 1.0; on.quantization = 1.0
                self.parameters = [on, FakeParameter()]
            @property
            def enabled(self): return self.parameters[0].value == 1.0
            @enabled.setter
            def enabled(self, _value): pass  # enable state is owned by the Device On parameter
        song = FakeSong(); nested = EnableableDevice(); nested.name = "Nested Utility"; sibling = FakeDevice(); sibling.name = "Sibling"
        chain_one = type("Chain", (), {"name": "Chain 1", "devices": [nested, sibling], "mute": False, "solo": False})(); chain_two = type("Chain", (), {"name": "Chain 2", "devices": [], "mute": False, "solo": False})()
        rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True; rack.chains = [chain_one, chain_two]
        song.tracks[0].devices = [rack]; mapper = LiveObjectMapper(song)
        track_ref = mapper.discover("track")["items"][0]["ref"]; top = mapper.discover("device", parent=track_ref)["items"]
        self.assertEqual([item["name"] for item in top], ["Rack"])
        nested_rows = mapper.discover("device", parent=top[0]["chains"][0]["ref"])["items"]; self.assertEqual([item["name"] for item in nested_rows], ["Nested Utility", "Sibling"])
        everywhere = mapper.discover("device", requested_fields=["name"])["items"]
        self.assertEqual({item["ref"] for item in top + nested_rows} <= {item["ref"] for item in everywhere}, True, "the whole Set's devices, nested ones too, without a parent")
        with self.assertRaisesRegex(ValueError, "parent reference is required"): mapper.discover("parameter")
        nested_row = nested_rows[0]; parameter = mapper.discover("parameter", parent=nested_row["ref"])["items"][0]
        self.assertEqual(parameter["parentRef"], nested_row["ref"]); self.assertEqual(mapper.get(nested_row["ref"])["name"], "Nested Utility")
        owner_identity = top[0]["chains"][0]["objectIdentity"]; siblings = [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in nested_rows]
        base = {"ref": nested_row["ref"], "expectedObjectIdentity": nested_row["objectIdentity"], "expectedOwnerRef": nested_row["parentRef"], "expectedOwnerIdentity": owner_identity, "expectedSiblings": siblings, "expectedTrackRef": track_ref, "expectedTrackIdentity": mapper.snapshot()["tracks"][0]["objectIdentity"]}
        changed = mapper.invoke("device.enable", {**base, "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"enabled": True}).encode()).hexdigest(), "enabled": False}); self.assertTrue(changed["changed"]); self.assertFalse(nested.enabled)
        base["expectedStateRevision"] = hashlib.sha256(mapper._bounded_canonical({"enabled": False}).encode()).hexdigest()
        replacement_sibling = FakeDevice(); replacement_sibling.name = "Sibling"; chain_one.devices[1] = replacement_sibling
        with self.assertRaises(ValueError): mapper.invoke("device.enable", {**base, "enabled": True})
        chain_one.devices[1] = sibling
        replacement_chain = type("Chain", (), {"name": "Replacement", "devices": [nested], "mute": False, "solo": False})(); rack.chains[0] = replacement_chain
        with self.assertRaises(ValueError): mapper.invoke("device.enable", {**base, "enabled": True})
        rack.chains[0] = chain_one; chain_one.devices = []; chain_two.devices = [nested]
        with self.assertRaises(ValueError): mapper.invoke("device.enable", {**base, "enabled": True})

    def test_parameter_rows_show_the_value_as_lives_panel_does(self):
        song = FakeSong(); parameter = song.tracks[0].devices[0].parameters[0]
        parameter.display_value = 20000.0; parameter.str_for_value = lambda value: "20.0 kHz"
        mapper = LiveObjectMapper(song)
        track = mapper.discover("track")["items"][0]
        device = mapper.discover("device", parent=track["ref"])["items"][0]
        self.assertEqual(mapper.discover("parameter", parent=device["ref"])["items"][0]["displayValue"], "20.0 kHz")
        parameter.str_for_value = lambda value: 1 / 0
        self.assertEqual(mapper.discover("parameter", parent=device["ref"])["items"][0]["displayValue"], "20000.0", "Live 12's number when the text fails")
        del parameter.str_for_value; del parameter.display_value
        self.assertEqual(mapper.discover("parameter", parent=device["ref"])["items"][0]["displayValue"], "0.5", "the value itself otherwise")

    def test_device_parameter_discovery_and_guarded_mutation(self):
        mapper = LiveObjectMapper(FakeSong())
        track = mapper.discover("track")["items"][0]
        device = mapper.discover("device", parent=track["ref"])["items"][0]
        parameter = mapper.discover("parameter", parent=device["ref"])["items"][0]
        self.assertEqual(parameter["parentRef"], device["ref"])
        self.assertEqual(parameter["revision"], 1)
        changed = mapper.invoke("device.parameter.set", {"ref": parameter["ref"], "value": 0.75, "expectedRevision": 1, **self.parameter_authority(mapper, parameter["ref"])})
        self.assertEqual(changed["value"], 0.75)
        self.assertEqual(changed["revision"], 2)
        with self.assertRaises(ValueError):
            mapper.invoke("device.parameter.set", {"ref": parameter["ref"], "value": 0.7})

    def test_several_parameters_of_a_device_change_in_one_request_all_or_none(self):
        song = FakeSong(); device = song.tracks[0].devices[0]
        device.parameters = [FakeParameter() for _ in range(3)]
        for index, parameter in enumerate(device.parameters): parameter.name = f"P{index}"
        mapper = LiveObjectMapper(song)
        track = mapper.discover("track")["items"][0]
        device_row = mapper.discover("device", parent=track["ref"])["items"][0]
        rows = mapper.discover("parameter", parent=device_row["ref"])["items"]
        authority = self.parameter_authority(mapper, rows[0]["ref"])
        shared = {key: value for key, value in authority.items() if key != "expectedObjectIdentity"}
        def item(row, value): return {"ref": row["ref"], "value": value, "expectedRevision": row["revision"], "expectedObjectIdentity": self.parameter_authority(mapper, row["ref"])["expectedObjectIdentity"]}
        self.assertTrue(mapper._operation_supported("device.parameters.set"))
        result = mapper.invoke("device.parameters.set", {**shared, "parameters": [item(rows[0], 0.75), item(rows[2], 0.25)]})
        validate_operation_payload("device.parameters.set", "result", result)
        self.assertEqual([parameter.value for parameter in device.parameters], [0.75, 0.5, 0.25])
        self.assertEqual([row["revision"] for row in result["parameters"]], [2, 2])
        rows = mapper.discover("parameter", parent=device_row["ref"])["items"]
        device.parameters[2].is_enabled = False
        with self.assertRaisesRegex(ValueError, "parameter 2 of 2: parameter is greyed out in Live right now"):
            mapper.invoke("device.parameters.set", {**shared, "parameters": [item(rows[1], 0.75), item(rows[2], 0.5)]})
        device.parameters[2].is_enabled = True
        self.assertEqual([parameter.value for parameter in device.parameters], [0.75, 0.5, 0.25], "the first one went back")
        stale = item(rows[0], 0.5); stale["expectedRevision"] = 1
        with self.assertRaisesRegex(ValueError, "revision changed since preview"):
            mapper.invoke("device.parameters.set", {**shared, "parameters": [item(rows[1], 0.25), stale]})
        self.assertEqual(device.parameters[1].value, 0.5, "every parameter is checked before any changes")
        with self.assertRaisesRegex(ValueError, "same parameter twice"):
            mapper.invoke("device.parameters.set", {**shared, "parameters": [item(rows[1], 0.25), item(rows[1], 0.5)]})

    def test_capabilities_are_derived_from_negotiated_operation_sets(self):
        mapper = LiveObjectMapper(FakeSong()); status = mapper.status(); operations, capabilities = set(status["operations"]), set(status["capabilities"])
        requirements = {
            "transport": {"transport.set", "tempo.set"}, "subscriptions": {"subscribe"},
            "session.midi_clip.create": {"clip.create"}, "session.midi_clip.delete": {"clip.delete"},
            "session.midi_note.write": {"note.add", "note.add-batch"},
        }
        for capability, required in requirements.items():
            if capability in capabilities: self.assertTrue(required <= operations, (capability, required - operations))
        self.assertNotIn("max", capabilities)

    def test_partial_recording_and_empty_parameter_shapes_are_not_overadvertised(self):
        partial = FakeSong(); del partial.record_mode; status = LiveObjectMapper(partial).status()
        self.assertNotIn("recording.session", status["operations"]); self.assertNotIn("recording", status["capabilities"])
        empty_device = FakeSong(); empty_device.tracks[0].devices[0].parameters = []; status = LiveObjectMapper(empty_device).status()
        self.assertIn("devices", status["capabilities"]); self.assertNotIn("parameters", status["capabilities"]); self.assertNotIn("device.parameter.write", status["capabilities"])

    def test_selection_uses_canonical_dereferenceable_track_identity(self):
        song = FakeSong(); track, scene, slot = song.tracks[0], song.scenes[0], song.tracks[0].clip_slots[0]; track._live_ptr = 101; scene._live_ptr = 102; slot._live_ptr = 103; copier = __import__("copy").copy
        song.view = type("View", (), {"selected_track": copier(track), "selected_scene": copier(scene), "highlighted_clip_slot": copier(slot)})()
        mapper = LiveObjectMapper(song); selection = mapper.discover("selection")["items"][0]
        canonical_track = mapper.discover("track")["items"][0]["ref"]
        self.assertEqual(selection["selectedTrackRef"], canonical_track); self.assertEqual(mapper.get(selection["selectedTrackRef"])["name"], "Drums")
        self.assertEqual(selection["selectedSceneRef"], mapper.discover("scene")["items"][0]["ref"]); self.assertTrue(selection["highlightedClipSlotRef"].endswith(":0:0"))

    def test_selection_reports_focus_in_plain_names(self):
        song = FakeSong(); track = song.tracks[0]; track.color = 0xFF6F61; track.has_midi_input = True
        device = FakeDevice(); device.name = "Operator"; track.devices = [device]
        track.view = type("TrackView", (), {"selected_device": device})()
        parameter = device.parameters[0]; parameter.canonical_parent = device; parameter.str_for_value = lambda value: f"{value:.2f} dB"
        clip = type("Clip", (), {"name": "Verse", "get_selected_notes_extended": lambda self: [1, 2]})()
        song.view = type("View", (), {"selected_track": track, "selected_scene": song.scenes[0], "highlighted_clip_slot": None, "detail_clip": clip, "selected_parameter": parameter, "selected_chain": None})()
        app_view = type("AppView", (), {"focused_document_view": "Arranger", "is_view_visible": lambda self, name: name in {"Detail/DeviceChain", "Browser"}})()
        mapper = LiveObjectMapper(song)
        with patch.object(LiveObjectMapper, "_application", lambda self: type("App", (), {"view": app_view})()):
            row = mapper.discover("selection")["items"][0]
        self.assertEqual({key: value for key, value in row.items() if key.startswith("focus")}, {
            "focusTrackName": "Drums", "focusTrackColor": "#ff6f61", "focusTrackKind": "midi", "focusSceneName": song.scenes[0].name or None,
            "focusClipName": "Verse", "focusDeviceName": "Operator", "focusParameterName": "Gain", "focusParameterValue": "0.50 dB",
            "focusParameterOwner": "Operator", "focusChainName": None, "focusView": "Arrangement", "focusDetail": "Device",
            "focusBrowser": True, "focusSelectedNotes": 2,
        })
        # Without Live's application view (as in these fakes), the view fields are simply unknown.
        self.assertIsNone(mapper.discover("selection")["items"][0]["focusView"])

    def test_proxy_identity_selection_tracks_recording_and_ambiguity_fail_closed(self):
        copier = __import__("copy").copy; song = FakeSong(); destination = song.tracks[0]; destination._live_ptr = 201; destination.arm = True; mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); destination_ref = snapshot["tracks"][0]["ref"]; fresh = copier(destination); song.tracks = [fresh]
        args = {"action": "start", "expectedSessionRecord": False, "expectedArrangementRecord": False, "destinationTrackRef": destination_ref, "destinationTrackIdentity": "live:201", "outputSafety": {"safe": True, "provenance": "unit-test"}}
        self.assertEqual(mapper._recording_authority(args, "session"), "start")
        song.tracks.append(copier(destination))
        with self.assertRaisesRegex(ValueError, "ambiguous"): mapper._recording_authority(args, "session")
        song = FakeSong(); song.return_tracks = [FakeTrack()]; song.master_track = FakeTrack(); mapper = LiveObjectMapper(song); rows = mapper.snapshot()["tracks"]
        for row in rows[1:]: self.assertEqual(mapper.get(row["ref"])["objectIdentity"], row["objectIdentity"])
        returned = rows[1]; mapper.invoke("track.rename", {"ref": returned["ref"], "name": "Return Renamed", "expectedName": returned["name"], "expectedObjectIdentity": returned["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", returned["ref"])})
        self.assertEqual(song.return_tracks[0].name, "Return Renamed")

    def test_recording_starts_with_other_tracks_armed_too_and_needs_the_destination_armed(self):
        song = FakeSong(); song.tracks = [FakeTrack(), FakeTrack(), FakeTrack()]
        for index, track in enumerate(song.tracks): track._live_ptr = 400 + index; track.arm = index < 2
        mapper = LiveObjectMapper(song); rows = mapper.snapshot()["tracks"]
        args = {"action": "start", "expectedSessionRecord": False, "expectedArrangementRecord": False, "destinationTrackRef": rows[0]["ref"], "destinationTrackIdentity": "live:400", "outputSafety": {"safe": True, "provenance": "unit-test"}}
        # Another track armed too: Live records onto both, as when the producer presses Record.
        self.assertEqual(mapper._recording_authority(args, "arrangement"), "start")
        both = {**args, "alsoTrackRefs": [rows[1]["ref"]], "alsoTrackIdentities": ["live:401"]}
        self.assertEqual(mapper._recording_authority(both, "arrangement"), "start")
        song.tracks[2].arm = True
        self.assertEqual(mapper._recording_authority(both, "arrangement"), "start")
        song.tracks[1].arm = False; song.tracks[2].arm = False
        with self.assertRaisesRegex(ValueError, "not armed"): mapper._recording_authority(both, "arrangement")
        song.tracks[0].arm = False
        with self.assertRaisesRegex(ValueError, "isn't armed"): mapper._recording_authority(args, "arrangement")

    def test_duplicate_proxy_identities_and_route_labels_are_refused(self):
        song = FakeSong(); first, second = FakeDevice(), FakeDevice(); first._live_ptr = 301; second._live_ptr = 302; song.tracks[0].devices = [first, second]; mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); track = snapshot["tracks"][0]; device = track["devices"][0]; siblings = [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in track["devices"]]
        second._live_ptr = 301
        with self.assertRaisesRegex(ValueError, "ambiguous"): mapper._device_location(device["ref"], device["objectIdentity"], track["ref"], track["objectIdentity"], siblings, track["ref"], track["objectIdentity"])
        choice_one, choice_two = FakeRouteChoice("Duplicate"), FakeRouteChoice("Duplicate"); song.tracks[0].available_input_routing_types = [choice_one, choice_two]
        with self.assertRaisesRegex(ValueError, "ambiguous"): mapper._routing_choice(song.tracks[0], "available_input_routing_types", "Duplicate")
        rack = FakeDevice(); rack.can_have_chains = True; chain = type("Chain", (), {})(); chain.devices = [rack]; rack.chains = [chain]; song.tracks[0].devices = [rack]
        with self.assertRaisesRegex(ValueError, "cyclic"): LiveObjectMapper(song).snapshot()

    def test_destructive_cleanup_requires_unforgeable_creation_ownership_of_the_same_object(self):
        song = FakeSong(); mapper = LiveObjectMapper(song); transaction = "structure-ownership-transaction"; created = mapper.invoke("track.create", {"name": "Owned", "kind": "midi", "index": 1, "expectedStructureRevision": mapper._structure_revision()}, transaction)
        self.assertIn("ownershipToken", created)
        delete_args = {"ref": created["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": created["objectIdentity"]}
        with self.assertRaisesRegex(ValueError, "transaction-owned"): mapper.invoke("track.delete", delete_args, "attacker-transaction")
        song.tracks[1].arrangement_clips = [FakeClip(4.0)]; delete_args["expectedStructureRevision"] = mapper._structure_revision()
        # A track it made goes though it changed since (recorded onto), only with the creating transaction's authority.
        with self.assertRaisesRegex(ValueError, "transaction-owned"): mapper.invoke("track.delete", delete_args, "attacker-transaction")
        self.assertEqual(mapper.invoke("track.delete", delete_args, transaction, created["ownershipToken"]), {"deleted": created["ref"]})
        clean_song = FakeSong(); clean_mapper = LiveObjectMapper(clean_song); clean = clean_mapper.invoke("scene.create", {"name": "Owned Scene", "index": 1, "expectedStructureRevision": clean_mapper._structure_revision()}, transaction); clean_args = {"ref": clean["ref"], "expectedStructureRevision": clean_mapper._structure_revision(), "expectedObjectIdentity": clean["objectIdentity"]}
        self.assertEqual(clean_mapper.invoke("scene.delete", clean_args, transaction, clean["ownershipToken"]), {"deleted": clean["ref"]}); clean_mapper._require_cleanup_ownership("scene.delete", clean_args, transaction, clean["ownershipToken"]); clean_mapper.retire_transaction_ownership(transaction)
        with self.assertRaisesRegex(ValueError, "transaction-owned"): clean_mapper._require_cleanup_ownership("scene.delete", clean_args, transaction, clean["ownershipToken"])
        collision_mapper = LiveObjectMapper(FakeSong()); first = collision_mapper.invoke("track.create", {"name": "First at zero", "kind": "midi", "index": 0, "expectedStructureRevision": collision_mapper._structure_revision()}, transaction)
        with self.assertRaisesRegex(ValueError, "shift active transaction-owned reference"): collision_mapper.invoke("track.create", {"name": "Second at zero", "kind": "midi", "index": 0, "expectedStructureRevision": collision_mapper._structure_revision()}, transaction)
        self.assertEqual(len(collision_mapper.song.tracks), 2); self.assertEqual(collision_mapper.invoke("track.delete", {"ref": first["ref"], "expectedStructureRevision": collision_mapper._structure_revision(), "expectedObjectIdentity": first["objectIdentity"]}, transaction, first["ownershipToken"]), {"deleted": first["ref"]}); self.assertEqual(len(collision_mapper.song.tracks), 1)
        shifted_mapper = LiveObjectMapper(FakeSong()); later = shifted_mapper.invoke("track.create", {"name": "Owned later", "kind": "midi", "index": 1, "expectedStructureRevision": shifted_mapper._structure_revision()}, transaction)
        # Another transaction adding a track before it is not refused: the older creation's reference
        # moved, so it loses its ownership, and its cleanup is refused instead.
        before = shifted_mapper.invoke("track.create", {"name": "Added before", "kind": "midi", "index": 0, "expectedStructureRevision": shifted_mapper._structure_revision()}, "other-structure-transaction")
        self.assertEqual([track.name for track in shifted_mapper.song.tracks], ["Added before", "Drums", "Owned later"])
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): shifted_mapper.invoke("track.delete", {"ref": later["ref"], "expectedStructureRevision": shifted_mapper._structure_revision(), "expectedObjectIdentity": later["objectIdentity"]}, transaction, later["ownershipToken"])
        self.assertEqual(len(shifted_mapper.song.tracks), 3)
        self.assertEqual(shifted_mapper.invoke("track.delete", {"ref": before["ref"], "expectedStructureRevision": shifted_mapper._structure_revision(), "expectedObjectIdentity": before["objectIdentity"]}, "other-structure-transaction", before["ownershipToken"]), {"deleted": before["ref"]})
        midi_mapper = LiveObjectMapper(FakeSong(), provenance="real-live"); track_ref = midi_mapper.snapshot()["tracks"][0]["ref"]; midi = midi_mapper.invoke("clip.create", self.clip_creation_args(midi_mapper, track_ref, 0, kind="midi", name="Owned MIDI", length=4), transaction); midi_mapper.invoke("note.add-batch", {"ref": midi["ref"], "notes": [{"pitch": 36, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1}], **self.note_authority(midi_mapper, midi["ref"])}, transaction); self.assertEqual(midi_mapper.invoke("clip.delete", {"ref": midi["ref"], **midi_mapper._session_clip_authority(midi["ref"])}, transaction, midi["ownershipToken"]), {"deleted": midi["ref"]})

    def test_an_owned_device_gone_from_the_set_no_longer_blocks_new_tracks(self):
        # Live replaced it, or the producer deleted it by hand: its cleanup can't happen, and tracks can still be added.
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live"); song.tracks.append(FakeTrack()); song.tracks[1].devices = []
        mapper._owned_cleanup_tokens["gone"] = {"transactionId": "t", "ref": f"{mapper.refs.epoch}:device:1:0", "objectIdentity": "live:vanished", "fingerprint": "0" * 64}
        self.assertFalse(mapper._owned_positional_conflict("track", 1))
        kept = song.tracks[0].devices[0]; mapper._owned_cleanup_tokens["kept"] = {"transactionId": "t", "ref": f"{mapper.refs.epoch}:device:0:0", "objectIdentity": mapper._capture_object_identity(kept), "fingerprint": "0" * 64}
        self.assertTrue(mapper._owned_positional_conflict("track", 0))

    def test_scene_capture_before_an_owned_scene_retires_its_ownership_instead_of_refusing(self):
        mapper = LiveObjectMapper(FakeSong()); transaction = "owned-scene-shift-transaction"; owned = mapper.invoke("scene.create", {"name": "Owned later", "index": 1, "expectedStructureRevision": mapper._structure_revision()}, transaction); mapper.song.capture_and_insert_scene = lambda: mapper.song.scenes.insert(0, FakeScene("Captured before"))
        captured = mapper.invoke("scene.capture", {"expectedStateRevision": mapper._capture_authority_revision()}, "capture-other-transaction")
        self.assertEqual([scene.name for scene in mapper.song.scenes], ["Captured before", "Scene 1", "Owned later"])
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper.invoke("scene.delete", {"ref": owned["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": owned["objectIdentity"]}, transaction, owned["ownershipToken"])
        self.assertEqual(mapper.invoke("scene.delete", {"ref": captured["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": captured["objectIdentity"]}, "capture-other-transaction", captured["ownershipToken"]), {"deleted": captured["ref"]})
        # The capturing transaction's own later scene still refuses a capture that would move it.
        own = mapper.invoke("scene.create", {"name": "Own later", "index": 2, "expectedStructureRevision": mapper._structure_revision()}, "capture-own-transaction")
        with self.assertRaisesRegex(ValueError, "shift active transaction-owned"): mapper.invoke("scene.capture", {"expectedStateRevision": mapper._capture_authority_revision()}, "capture-own-transaction")
        self.assertEqual([scene.name for scene in mapper.song.scenes], ["Scene 1", "Owned later", "Own later"]); self.assertTrue(own["ownershipToken"])

    def test_session_structure_lifecycle_and_empty_slots_are_authoritative(self):
        mapper = LiveObjectMapper(FakeSong())
        track = mapper.discover("track")["items"][0]
        self.assertTrue(track["clipSlots"][0]["empty"])
        stale_revision = mapper._structure_revision(); mapper.song.scenes.append(FakeScene("External"))
        with self.assertRaises(ValueError): mapper.invoke("track.create", {"name": "Stale", "kind": "midi", "index": 1, "expectedStructureRevision": stale_revision})
        mapper.song.scenes.pop()
        created_track = mapper.invoke("track.create", {"name": "Strings", "kind": "midi", "index": 1, "expectedStructureRevision": mapper._structure_revision()})
        created_scene = mapper.invoke("scene.create", {"name": "Verse", "index": 1, "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual(created_track["name"], "Strings"); self.assertTrue(created_track["objectIdentity"])
        self.assertEqual(created_scene["name"], "Verse"); self.assertTrue(created_scene["objectIdentity"])
        self.assertEqual(mapper.invoke("track.rename", {"ref": created_track["ref"], "name": "Synths", "expectedName": "Strings", "expectedObjectIdentity": created_track["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", created_track["ref"])})["name"], "Synths")
        with self.assertRaises(ValueError): mapper.invoke("track.rename", {"ref": created_track["ref"], "name": "Wrong", "expectedName": "Strings", "expectedObjectIdentity": created_track["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", created_track["ref"])})
        self.assertEqual(mapper.invoke("scene.rename", {"ref": created_scene["ref"], "name": "Chorus", "expectedName": "Verse", "expectedObjectIdentity": created_scene["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("scene", created_scene["ref"])})["name"], "Chorus")
        created_track_object = mapper.song.tracks[1]; replacement = FakeTrack(); replacement.name = "Synths"; mapper.song.tracks[1] = replacement
        with self.assertRaises(ValueError): mapper.invoke("track.delete", {"ref": created_track["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": created_track["objectIdentity"]})
        mapper.song.tracks[1] = created_track_object
        self.assertEqual(mapper.invoke("track.delete", {"ref": created_track["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": created_track["objectIdentity"]}), {"deleted": created_track["ref"]})
        self.assertEqual(mapper.invoke("scene.delete", {"ref": created_scene["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": created_scene["objectIdentity"]}), {"deleted": created_scene["ref"]})

    def test_structure_operations_fail_closed_when_live_shape_is_unsupported(self):
        class UnsupportedSong:
            tracks = []
            scenes = []
        mapper = LiveObjectMapper(UnsupportedSong())
        with self.assertRaises(ValueError):
            mapper.invoke("track.create", {"name": "Nope", "kind": "midi", "index": 0, "expectedStructureRevision": mapper._structure_revision()})

    def test_status_does_not_advertise_mutations_missing_from_observed_live_shape(self):
        class ReadOnlyTrack:
            clip_slots = []
            devices = []

        class ReadOnlySong:
            tracks = [ReadOnlyTrack()]
            scenes = []

        status = LiveObjectMapper(ReadOnlySong()).status()
        self.assertIn("status", status["operations"])
        self.assertIn("discover", status["operations"])
        self.assertNotIn("track.create", status["operations"])
        self.assertNotIn("scene.create", status["operations"])
        self.assertNotIn("clip.create", status["operations"])
        self.assertNotIn("note.add", status["operations"])
        self.assertNotIn("note.add-batch", status["operations"])
        self.assertNotIn("device.parameter.set", status["operations"])
        self.assertNotIn("locator.add", status["operations"])

    def test_status_requires_callable_delete_and_usable_device_parameters(self):
        class EmptyDevice:
            parameters = []

        class ReadOnlyTrack:
            clip_slots = []
            devices = [EmptyDevice()]

        class ReadOnlySong:
            tracks = [ReadOnlyTrack()]
            scenes = []

        status = LiveObjectMapper(ReadOnlySong()).status()
        self.assertNotIn("track.delete", status["operations"])
        self.assertNotIn("device.parameter.set", status["operations"])

    def test_hierarchical_discovery_exposes_song_parents_and_empty_slots(self):
        mapper = LiveObjectMapper(FakeSong())
        song = mapper.discover("song")["items"][0]
        track = mapper.discover("track")["items"][0]
        slot = mapper.discover("clip_slot", parent=track["ref"])["items"][0]
        self.assertEqual(track["parentRef"], song["ref"])
        self.assertEqual(slot["parentRef"], track["ref"])
        self.assertTrue(slot["empty"])
        self.assertEqual(mapper.discover("clip_slot", parent=track["ref"], requested_fields=["empty"])["items"][0], {"ref": slot["ref"], "parentRef": track["ref"], "empty": True})

    def test_discovery_cursor_is_opaque_authenticated_and_revision_bound(self):
        mapper = LiveObjectMapper(FakeSong())
        page = mapper.discover("track", limit=1, traversal_budget=1)
        self.assertIsNone(page.get("nextCursor"))
        mapper = LiveObjectMapper(FakeSong())
        mapper.song.tracks.append(FakeTrack())
        page = mapper.discover("track", limit=1)
        cursor = page["nextCursor"]
        self.assertIsInstance(cursor, str)
        self.assertNotIn(":", cursor)
        self.assertEqual(len(mapper.discover("track", limit=1, cursor=cursor)["items"]), 1)
        tampered = ("A" if cursor[4] != "A" else "B") + cursor[5:]
        with self.assertRaises(ValueError):
            mapper.discover("track", limit=1, cursor=tampered)

    def test_discovery_rejects_stale_parent_and_reports_unknown_playback_authoritatively(self):
        mapper = LiveObjectMapper(FakeSong())
        stale = f"{mapper.refs.epoch + 1}:track:0"
        with self.assertRaises(ValueError):
            mapper.discover("clip_slot", parent=stale)
        playback = mapper.discover("session_playback")["items"][0]
        self.assertIs(playback["transport"]["playing"], False)
        self.assertEqual(playback["transport"]["launchQuantization"]["normalized"], "1-bar")
        self.assertEqual(playback["firedTargets"], [])

    def test_playback_derives_exact_targets_from_track_slot_indexes(self):
        song = FakeSong()
        song.tracks[0].clip_slots[0].create_clip(4)
        song.tracks[0].playing_slot_index = 0
        song.tracks[0].fired_slot_index = 0
        mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        track = snapshot["tracks"][0]
        self.assertEqual(track["monitoringState"], "off")
        self.assertEqual(track["playingSlotIndex"], 0)
        for target in snapshot["playback"]["playingTargets"] + snapshot["playback"]["firedTargets"]:
            self.assertEqual(target["trackRef"], track["ref"])
            self.assertEqual(target["clipSlotRef"], track["clipSlots"][0]["ref"])
            self.assertEqual(target["sceneRef"], snapshot["scenes"][0]["ref"])
            self.assertEqual(target["sceneIndex"], 0)
            self.assertEqual(target["clipRef"], track["clipSlots"][0]["clipRef"])

    def test_unknown_monitoring_and_arm_remain_unavailable(self):
        song = FakeSong()
        del song.tracks[0].arm

    def _audition_args(self, mapper):
        snapshot = mapper.snapshot()
        scene = snapshot["scenes"][0]
        track = snapshot["tracks"][0]
        slot = track["clipSlots"][0]
        return snapshot, {
            "ref": scene["ref"],
            "setName": snapshot["set"]["name"],
            "sceneName": scene["name"],
            "sceneIndex": scene["index"],
            "playbackRevision": snapshot["playback"]["revision"],
            "eligibleTargets": [f"{track['ref']}|{slot['ref']}|{scene['ref']}"],
            "expectedSetIdentity": snapshot["set"]["objectIdentity"],
            "expectedAuthorityRevision": mapper._audition_authority_revision(snapshot, scene["ref"], scene["index"], {f"{track['ref']}|{slot['ref']}|{scene['ref']}"}),
            "outputSafety": {"safe": True, "provenance": "unit-test-operator"},
        }

    def test_guarded_audition_launch_rechecks_identity_safety_and_eligibility(self):
        mapper = LiveObjectMapper(FakeAuditionSong())
        snapshot, args = self._audition_args(mapper)
        self.assertIn("session.audition-launch", mapper.status()["operations"])
        self.assertIn("session.audition-stop", mapper.status()["operations"])
        self.assertIn("session.emergency-stop", mapper.status()["operations"])
        self.assertNotIn("scene.launch", mapper.status()["operations"])
        self.assertNotIn("stop-all-clips", mapper.status()["operations"])
        self.assertNotIn("transport.stop", mapper.status()["operations"])
        unsafe = dict(args); unsafe.pop("outputSafety")
        with self.assertRaises(ValueError): mapper.invoke("session.audition-launch", unsafe)
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", {**args, "playbackRevision": "stale"})
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", {**args, "setName": "Other Set"})
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", {**args, "eligibleTargets": [f"{snapshot['tracks'][0]['ref']}|{snapshot['tracks'][0]['clipSlots'][0]['ref']}|1:scene:9"]})
        result = mapper.invoke("session.audition-launch", args)
        self.assertEqual(result["launched"], args["ref"])
        self.assertEqual(len(result["targets"]), 1)
        self.assertTrue(mapper.song.is_playing)
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", args)

    def test_guarded_audition_launch_refuses_armed_or_monitored_tracks(self):
        song = FakeAuditionSong()
        song.tracks[0].arm = True
        mapper = LiveObjectMapper(song)
        _, args = self._audition_args(mapper)
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", args)
        song.tracks[0].arm = False
        song.tracks[0].current_monitoring_state = 0
        mapper = LiveObjectMapper(song)
        _, args = self._audition_args(mapper)
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-launch", args)
        # Auto monitoring with a verified-unarmed track passes no input.
        song.tracks[0].current_monitoring_state = 1
        mapper = LiveObjectMapper(song)
        _, args = self._audition_args(mapper)
        result = mapper.invoke("session.audition-launch", args)
        self.assertEqual(result["launched"], args["ref"])

    def test_guarded_audition_stop_requires_owned_playback_and_verifies_stopped(self):
        mapper = LiveObjectMapper(FakeAuditionSong())
        _, launch = self._audition_args(mapper)
        mapper.invoke("session.audition-launch", launch)
        stop_args = {"ref": launch["ref"], "setName": launch["setName"], "eligibleTargets": launch["eligibleTargets"], "expectedSetIdentity": launch["expectedSetIdentity"], "expectedAuthorityRevision": launch["expectedAuthorityRevision"]}
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-stop", {**stop_args, "setName": "Other Set"})
        self.assertTrue(mapper.song.is_playing)
        self.assertEqual(mapper.invoke("session.audition-stop", stop_args), {"stopped": True})
        self.assertFalse(mapper.song.is_playing)
        self.assertEqual(mapper.song.stopped_all, 1)
        # Stopping again with no active playback is an idempotent no-op.
        self.assertEqual(mapper.invoke("session.audition-stop", stop_args), {"stopped": True})
        # External playback outside the owned target set refuses the owned stop.
        _, relaunch = self._audition_args(mapper)
        mapper.invoke("session.audition-launch", relaunch)
        scene2 = FakeScene("Scene 2")
        mapper.song.scenes.append(scene2)
        mapper.song.tracks[0].clip_slots.append(FakeSlot())
        with self.assertRaises(ValueError):
            mapper.invoke("session.audition-stop", {**stop_args, "eligibleTargets": []})
        self.assertTrue(mapper.song.is_playing)

    def test_guarded_emergency_stop_requires_exact_observation_and_stops(self):
        mapper = LiveObjectMapper(FakeAuditionSong())
        _, launch = self._audition_args(mapper)
        mapper.invoke("session.audition-launch", launch)
        with self.assertRaises(ValueError):
            mapper.invoke("session.emergency-stop", {"expectedTargets": [], "expectedRecording": "stopped"})
        self.assertTrue(mapper.song.is_playing)
        result = mapper.invoke("session.emergency-stop", {"expectedTargets": launch["eligibleTargets"], "expectedRecording": "stopped"})
        self.assertEqual(result["stopped"], True)
        self.assertEqual(result["stoppedTargets"], launch["eligibleTargets"])
        self.assertFalse(mapper.song.is_playing)
        # An empty observation is exact when nothing is playing.
        self.assertEqual(mapper.invoke("session.emergency-stop", {"expectedTargets": [], "expectedRecording": "stopped"})["stopped"], True)

    def test_generic_audible_operations_are_not_mapper_capabilities(self):
        mapper = LiveObjectMapper(FakeAuditionSong())
        operation_ids = {item["id"] for item in operation_registry()[0]["operations"]}
        for operation in ("set", "clip.launch", "track.stop", "playback.stop-all-clips", "scene.launch", "stop-all-clips", "transport.stop"):
            self.assertNotIn(operation, operation_ids)
            with self.assertRaises(ValueError):
                mapper.invoke(operation, {})

    def test_guarded_clip_launch_and_stop_require_exact_atomic_identity(self):
        song = FakeAuditionSong(); song.tracks[0].clip_slots[0].fire = song.scenes[0].fire; song.tracks[0].stop_all_clips = song.stop_all_clips
        second_slot = FakeSlot(); second_slot.clip = FakeClip(4.0); second_slot.fire = song.scenes[0].fire
        song.tracks[0].clip_slots.append(second_slot); song.scenes.append(FakeScene("Scene 2"))
        mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot(); track = snapshot["tracks"][0]; slot = track["clipSlots"][0]; scene = snapshot["scenes"][0]
        clip = next(item for item in track["clips"] if item["ref"] == slot["clipRef"])
        authority = {"slotRef": slot["ref"], "trackRef": track["ref"], "sceneRef": scene["ref"], "sceneIndex": scene["index"], "clipRef": slot["clipRef"], "trackIdentity": track["objectIdentity"], "sceneIdentity": scene["objectIdentity"], "slotIdentity": slot["objectIdentity"], "clipIdentity": clip["objectIdentity"], "playbackRevision": snapshot["playback"]["revision"], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}}
        # Playback moving on since the preview doesn't stop a launch (it launches whatever plays); a target that isn't the one previewed does.
        cross_wired = dict(authority); cross_wired["sceneRef"] = snapshot["scenes"][1]["ref"]; cross_wired["sceneIndex"] = 1
        with self.assertRaises(ValueError): mapper.invoke("session.clip-launch", cross_wired)
        launched = mapper.invoke("session.clip-launch", authority)
        self.assertEqual(launched["launched"], slot["ref"])
        # Launching again while it plays isn't refused, as pressing the slot again in Live.
        layered = dict(authority); layered["playbackRevision"] = mapper.snapshot()["playback"]["revision"]
        self.assertEqual(mapper.invoke("session.clip-launch", layered)["launched"], slot["ref"])
        stopped = mapper.invoke("session.clip-stop", {key: value for key, value in authority.items() if key != "playbackRevision"})
        self.assertTrue(stopped["stopped"])

    def test_guarded_clip_launch_accepts_fresh_live_proxies_but_not_replacements(self):
        song = FakeAuditionSong(); original_track, original_scene, original_slot, original_clip = song.tracks[0], song.scenes[0], song.tracks[0].clip_slots[0], song.tracks[0].clip_slots[0].clip
        for value, pointer in ((original_track, 101), (original_scene, 102), (original_slot, 103), (original_clip, 104)): value._live_ptr = pointer
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); track_row = snapshot["tracks"][0]; slot_row = track_row["clipSlots"][0]; scene_row = snapshot["scenes"][0]; clip_row = track_row["clips"][0]
        authority = {"slotRef": slot_row["ref"], "trackRef": track_row["ref"], "sceneRef": scene_row["ref"], "sceneIndex": 0, "clipRef": slot_row["clipRef"], "trackIdentity": track_row["objectIdentity"], "sceneIdentity": scene_row["objectIdentity"], "slotIdentity": slot_row["objectIdentity"], "clipIdentity": clip_row["objectIdentity"], "playbackRevision": snapshot["playback"]["revision"], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}}
        fresh_track, fresh_scene, fresh_slot, fresh_clip = FakeTrack(), FakeScene("Scene 1"), FakeSlot(), FakeClip(4.0)
        for value, pointer in ((fresh_track, 101), (fresh_scene, 102), (fresh_slot, 103), (fresh_clip, 104)): value._live_ptr = pointer
        fired = []
        def fresh_fire(): fired.append(True); song.is_playing = True; fresh_track.playing_slot_index = 0; fresh_track.fired_slot_index = 0
        def fresh_stop(): fresh_track.playing_slot_index = -1; fresh_track.fired_slot_index = -1
        fresh_slot.clip = fresh_clip; fresh_slot.fire = fresh_fire; fresh_track.stop_all_clips = fresh_stop; fresh_track.clip_slots = [fresh_slot]; song.tracks = [fresh_track]; song.scenes = [fresh_scene]
        self.assertEqual(mapper.invoke("session.clip-launch", authority)["launched"], slot_row["ref"]); self.assertEqual(fired, [True])
        stop_authority = {key: value for key, value in authority.items() if key not in {"playbackRevision", "outputSafety"}}
        replacement = FakeClip(4.0); replacement._live_ptr = 999; fresh_slot.clip = replacement
        with self.assertRaises(ValueError): mapper.invoke("session.clip-stop", stop_authority)
        self.assertEqual(fresh_track.playing_slot_index, 0)
        fresh_slot.clip = fresh_clip; self.assertTrue(mapper.invoke("session.clip-stop", stop_authority)["stopped"])
        song.is_playing = False; fired.clear(); fresh_slot.clip = replacement
        with self.assertRaises(ValueError): mapper.invoke("session.clip-launch", authority)
        self.assertEqual(fired, [])

    def test_recording_requires_atomic_state_destination_and_output_authority(self):
        song = FakeAuditionSong(); song.tracks[0].arm = True
        mapper = LiveObjectMapper(song); track_row = mapper.snapshot()["tracks"][0]; track_ref = track_row["ref"]
        authority = {"action": "start", "expectedSessionRecord": False, "expectedArrangementRecord": False, "destinationTrackRef": track_ref, "destinationTrackIdentity": track_row["objectIdentity"], "outputSafety": {"safe": True, "provenance": "operator-observed"}}
        with self.assertRaises(ValueError): mapper.invoke("recording.session", {**authority, "expectedSessionRecord": True})
        self.assertEqual(mapper.invoke("recording.session", authority)["recording"], True)
        with self.assertRaises(ValueError): mapper.invoke("recording.session", authority)
        stopped_recording = mapper.invoke("session.emergency-stop", {"expectedTargets": [], "expectedRecording": "session"})
        self.assertTrue(stopped_recording["recordingStopped"]); self.assertFalse(song.session_record)

    def test_a_track_live_cannot_arm_any_more_is_not_armed_and_does_not_block_recording(self):
        # Live keeps arm on for a track whose input became No Input (its source track deleted), but
        # can't arm or disarm it, and it records nothing.
        song = FakeAuditionSong(); song.tracks[0].arm = True
        orphan = copy.copy(song.tracks[0]); orphan.name = "Old Bounce"; orphan.arm = True; orphan.can_be_armed = False; song.tracks.append(orphan)
        mapper = LiveObjectMapper(song); rows = mapper.snapshot()["tracks"]
        self.assertEqual([row["armed"] for row in rows], [True, False])
        authority = {"action": "start", "expectedSessionRecord": False, "expectedArrangementRecord": False, "destinationTrackRef": rows[0]["ref"], "destinationTrackIdentity": rows[0]["objectIdentity"], "outputSafety": {"safe": True, "provenance": "operator-observed"}}
        self.assertEqual(mapper.invoke("recording.session", authority)["recording"], True)

    def test_unknown_monitoring_and_arm_remain_unavailable(self):
        song = FakeSong()
        del song.tracks[0].arm
        song.tracks[0].current_monitoring_state = 99
        row = LiveObjectMapper(song).snapshot()["tracks"][0]
        self.assertIsNone(row["armed"])
        self.assertIsNone(row["monitoringState"])

    def test_arrangement_locators_are_authoritative_and_reversible(self):
        song = FakeArrangementSong()
        mapper = LiveObjectMapper(song)
        self.assertIn("arrangement.write", mapper.status()["capabilities"])
        self.assertEqual(mapper.discover("locator")["items"][0]["name"], "Intro")
        create_args = {"name": "Verse", "position": 8, "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]}
        with self.assertRaisesRegex(ValueError, "playhead is moving; retry shortly"):
            mapper.invoke("arrangement.locator.create", create_args)
        created = mapper.invoke("arrangement.locator.create", create_args)
        self.assertEqual(created["name"], "Verse")
        self.assertEqual(mapper.discover("locator")["items"][-1]["position"], 8)
        delete_args = {"ref": created["ref"], "expectedObjectIdentity": created["objectIdentity"], "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]}
        self.assertEqual(mapper.invoke("arrangement.locator.delete", delete_args), {"deleted": created["ref"]})
        self.assertEqual([item["name"] for item in mapper.discover("locator")["items"]], ["Intro"])

    def test_arrangement_locator_rejects_collisions_and_unsupported_shapes(self):
        mapper = LiveObjectMapper(FakeArrangementSong())
        with self.assertRaises(ValueError):
            mapper.invoke("arrangement.locator.create", {"name": "Other", "position": 0, "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]})
        with self.assertRaises(ValueError):
            mapper.invoke("arrangement.locator.create", {"name": "Other", "position": float("nan")})
        with self.assertRaises(ValueError):
            LiveObjectMapper(FakeSong()).invoke("arrangement.locator.create", {"name": "Other", "position": 4})

    def test_entrypoint_requires_explicit_loopback_configuration(self):
        with self.assertRaises(ValueError):
            create_instance(FakeInstance())

    def test_bridge_rejects_ambient_environment_configuration(self):
        with self.assertRaises(ValueError):
            AbletonMcpBridge(FakeInstance())

    def test_real_live_mapper_reconnect_clears_cleanup_ownership_without_mutation_transaction(self):
        mapper = LiveObjectMapper(FakeSong(), provenance="real-live"); mapper._owned_cleanup_tokens["o" * 48] = {"transactionId": "transaction-one", "ref": "ref", "objectIdentity": "identity", "fingerprint": "f" * 64}
        result = mapper.invoke("session.reconnect", {})
        self.assertEqual(result["connected"], True); self.assertEqual(mapper._owned_cleanup_tokens, {})

    def test_mapper_get_is_read_only_and_generic_set_is_absent(self):
        mapper = LiveObjectMapper(FakeSong())
        track_ref = mapper.discover("track")["items"][0]["ref"]
        self.assertEqual(mapper.get(track_ref)["name"], "Drums")
        self.assertFalse(hasattr(mapper, "set"))

    def test_mapper_discovery_and_midi_lifecycle_use_fake_live_objects(self):
        mapper = LiveObjectMapper(FakeSong())
        status = mapper.status()
        self.assertTrue(status["connected"])
        self.assertIn("session.midi_clip.create", status["capabilities"])
        self.assertIn("session.midi_clip.delete", status["capabilities"])
        self.assertIn("session.midi_note.write", status["capabilities"])
        self.assertIn("note.add-batch", status["operations"])
        track = mapper.discover("track")["items"][0]["ref"]
        created = mapper.invoke("clip.create", self.clip_creation_args(mapper, track, 0, kind="midi", name="Four bars", length=16))
        self.assertEqual(created["name"], "Four bars")
        mapper.invoke("note.add", {"ref": created["ref"], "note": {"pitch": 36, "start": 0, "duration": 0.25, "velocity": 110, "channel": 1}, **self.note_authority(mapper, created["ref"])})
        batch = mapper.invoke("note.add-batch", {"ref": created["ref"], "notes": [
            {"pitch": 38, "start": 1, "duration": 0.25, "velocity": 100, "channel": 1},
            {"pitch": 42, "start": 2, "duration": 0.25, "velocity": 90, "channel": 1, "mute": True, "probability": 0.5, "velocityDeviation": 7, "releaseVelocity": 32},
        ], **self.note_authority(mapper, created["ref"])})
        self.assertEqual(batch["added"], 2); self.assertEqual(batch["noteIds"], [2, 3]); self.assertRegex(batch["notesRevision"], r"^[a-f0-9]{64}$")
        clip = mapper.refs.get(created["ref"])
        self.assertEqual([note["pitch"] for note in clip.get_notes(0, 0, 0, 128)], [36, 38, 42])
        expressive = mapper.get(created["ref"])["notes"][2]
        self.assertEqual({key: expressive[key] for key in ("mute", "probability", "velocityDeviation", "releaseVelocity")}, {"mute": True, "probability": 0.5, "velocityDeviation": 7.0, "releaseVelocity": 32.0})
        self.assertEqual(mapper.invoke("clip.delete", {"ref": created["ref"], **mapper._session_clip_authority(created["ref"])}), {"deleted": created["ref"]})

    def test_note_batch_ids_exclude_preexisting_coincident_notes(self):
        class ExtendedNote:
            def __init__(self, note_id, pitch, start, duration, velocity=100):
                self.note_id = note_id; self.pitch = pitch; self.start_time = start; self.duration = duration; self.velocity = velocity
                self.channel = 1; self.mute = False; self.probability = 1.0; self.velocity_deviation = 0.0; self.release_velocity = 64.0

        class ExtendedClip:
            length = 4.0
            def __init__(self): self.notes = [ExtendedNote(7, 36, 0, 0.25)]; self.next_id = 8
            def get_all_notes_extended(self): return list(self.notes)
            def add_new_notes(self, notes):
                for note in notes:
                    self.notes.append(ExtendedNote(self.next_id, note["pitch"], note["start_time"], note["duration"], note["velocity"])); self.next_id += 1
            def remove_notes_by_id(self, ids): self.notes = [note for note in self.notes if note.note_id not in set(ids)]

        song = FakeSong(); clip = ExtendedClip(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); clip_ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        result = mapper.invoke("note.add-batch", {"ref": clip_ref, "notes": [{"pitch": 36, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1}], **self.note_authority(mapper, clip_ref)})
        self.assertEqual(result["added"], 1); self.assertEqual(result["noteIds"], [8]); self.assertRegex(result["notesRevision"], r"^[a-f0-9]{64}$")

    def test_note_batch_accepts_float32_readback_and_preserves_existing_ids(self):
        class NativePrecisionClip(FakeClip):
            def add_new_notes(self, notes):
                super().add_new_notes(notes)
                for note in self.notes:
                    for field in ("start_time", "duration", "probability", "velocityDeviation", "releaseVelocity"):
                        if field in note: note[field] = struct.unpack("f", struct.pack("f", note[field]))[0]
                self.notes.sort(key=lambda note: note["start_time"])

        song = FakeSong(); clip = NativePrecisionClip(8); song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        note = {"pitch": 29, "start": 0.1, "duration": 0.35, "velocity": 110, "channel": 1, "probability": 0.7}
        first = mapper.invoke("note.add-batch", {"ref": ref, "notes": [note], **self.note_authority(mapper, ref)})
        before = mapper._read_notes(clip)
        second = mapper.invoke("note.add-batch", {"ref": ref, "notes": [note, {**note, "start": 1.5, "duration": 0.22}], **self.note_authority(mapper, ref)})
        self.assertEqual(first["noteIds"], [1]); self.assertEqual(second["noteIds"], [2, 3])
        self.assertEqual(mapper._read_notes(clip)[0], before[0])
        self.assertNotEqual(mapper._read_notes(clip)[0]["duration"], note["duration"])

        # Reversed requests and native time sorting must use float32 dictionary
        # buckets, without scanning the remaining notes for every request.
        from unittest.mock import patch
        import ableton_mcp_remote_script as bridge
        notes = [{**note, "start": (index + 1) / 1000, "duration": 1 / 3} for index in reversed(range(1000))]
        with patch.object(bridge, "_same_number", wraps=bridge._same_number) as compare:
            third = mapper.invoke("note.add-batch", {"ref": ref, "notes": notes, **self.note_authority(mapper, ref)})
            self.assertEqual(third["added"], 1000)
            self.assertLess(compare.call_count, 10)

    def test_note_batch_still_rejects_wrong_values_extra_notes_and_duplicate_ids(self):
        for corruption in ("duration", "pitch", "extra", "duplicate"):
            with self.subTest(corruption=corruption):
                class BadClip(FakeClip):
                    def add_new_notes(self, notes):
                        super().add_new_notes(notes)
                        if corruption == "duration": self.notes[-1]["duration"] += 0.01
                        elif corruption == "pitch": self.notes[-1]["pitch"] += 1
                        elif corruption == "extra": super().add_new_notes(notes)
                        else: self.notes.append(dict(self.notes[-1]))
                song = FakeSong(); clip = BadClip(4); song.tracks[0].clip_slots[0].clip = clip
                mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
                with self.assertRaisesRegex(ValueError, "exact complete expected state"):
                    mapper.invoke("note.add-batch", {"ref": ref, "notes": [{"pitch": 36, "start": 0, "duration": 0.35, "velocity": 100, "channel": 1}], **self.note_authority(mapper, ref)})
                self.assertEqual(clip.notes, [])

    def test_midi_reads_cover_exact_clip_length_and_refuse_unbounded_or_replacing_fallbacks(self):
        class LegacyClip:
            def __init__(self, count=1): self.length = 6000.0; self.calls = []; self.count = count; self.set_called = False
            def get_notes(self, pitch, start, span, pitches): self.calls.append((pitch, start, span, pitches)); return [(60, 5000.0, 0.25, 100)] * self.count
            def add_new_notes(self, _notes): pass
            def set_notes(self, _notes): self.set_called = True
        song = FakeSong(); clip = LegacyClip(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertEqual(row["notes"][0]["start"], 5000.0); self.assertEqual(clip.calls[-1][2], 6000.0)
        authority = self.note_authority(mapper, row["ref"])
        with self.assertRaisesRegex(ValueError, "set_notes replacement is refused"): mapper.invoke("note.add-batch", {"ref": row["ref"], "notes": [{"pitch": 61, "start": 1, "duration": 0.25, "velocity": 100, "channel": 1}], **authority})
        self.assertFalse(clip.set_called)
        oversized = LegacyClip(513); song.tracks[0].clip_slots[0].clip = oversized
        with patch.object(remote_module, "MAX_WIRE_ARRAY_LENGTH", 512), self.assertRaisesRegex(ValueError, "exceeds its authoritative bound"): LiveObjectMapper(song).snapshot()

    def test_partial_native_note_addition_rolls_back_new_stable_ids(self):
        class Note:
            def __init__(self, note_id, pitch): self.note_id = note_id; self.pitch = pitch; self.start_time = 0.0; self.duration = 0.25; self.velocity = 100; self.channel = 1; self.mute = False; self.probability = 1.0; self.velocity_deviation = 0.0; self.release_velocity = 64.0
        class Clip:
            length = 4.0
            def __init__(self): self.notes = [Note(1, 36)]
            def get_all_notes_extended(self): return list(self.notes)
            def add_new_notes(self, notes): self.notes.append(Note(2, notes[0]["pitch"])); raise RuntimeError("injected partial native add")
            def remove_notes_by_id(self, ids): self.notes = [note for note in self.notes if note.note_id not in ids]
        song = FakeSong(); clip = Clip(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        with self.assertRaisesRegex(RuntimeError, "partial native add"): mapper.invoke("note.add-batch", {"ref": ref, "notes": [{"pitch": 38, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1}], **self.note_authority(mapper, ref)})
        self.assertEqual([note.note_id for note in clip.notes], [1])
        duplicate = Clip(); duplicate.notes = [Note(1, 36), Note(1, 38)]; song.tracks[0].clip_slots[0].clip = duplicate; mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "unique stable note identity"): mapper.invoke("note.add-batch", {"ref": ref, "notes": [{"pitch": 40, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1}], **self.note_authority(mapper, ref)})
        self.assertEqual(len(duplicate.notes), 2)

    def test_mapper_clip_creation_uses_session_slot_index(self):
        mapper = LiveObjectMapper(FakeSong())
        track = mapper.discover("track")["items"][0]["ref"]
        created = mapper.invoke("clip.create", self.clip_creation_args(mapper, track, 0, kind="midi", name="Session slot", length=16))
        self.assertEqual(mapper.refs.get(created["ref"]).length, 16)

    def test_post_creation_mapping_failures_remove_owned_clip_and_device(self):
        song = FakeSong(); mapper = LiveObjectMapper(song); track_ref = mapper.snapshot()["tracks"][0]["ref"]; mapper._mapped_fingerprint = lambda _reference: (_ for _ in ()).throw(RuntimeError("injected mapping failure"))
        with self.assertRaisesRegex(RuntimeError, "injected mapping failure"): mapper.invoke("clip.create", self.clip_creation_args(mapper, track_ref, 0, kind="midi", name="Temporary", length=4))
        self.assertIsNone(song.tracks[0].clip_slots[0].clip)
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.insert_device = lambda name, index: track.devices.insert(len(track.devices) if index < 0 else index, type("InsertedDevice", (), {"name": name, "class_name": "InsertedDevice", "enabled": True, "parameters": [FakeParameter()]})()); track.delete_device = lambda index: track.devices.pop(index); mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; mapper._mapped_fingerprint = lambda _reference: (_ for _ in ()).throw(RuntimeError("injected device mapping failure"))
        with self.assertRaisesRegex(RuntimeError, "injected device mapping failure"): mapper.invoke("device.insert", {"trackRef": row["ref"], "deviceName": "Utility", "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in row["devices"]]})
        self.assertEqual(len(track.devices), 0)
        song = FakeSong(); slot = song.tracks[0].clip_slots[0]; slot.create_clip = lambda length: (setattr(slot, "clip", FakeClip(length + 1)) or slot.clip); mapper = LiveObjectMapper(song); track_ref = mapper.snapshot()["tracks"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "name or length"): mapper.invoke("clip.create", self.clip_creation_args(mapper, track_ref, 0, kind="midi", name="Wrong length", length=4))
        self.assertIsNone(slot.clip)
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.insert_device = lambda name, index: track.devices.append(type("InsertedDevice", (), {"name": "Substituted", "class_name": "InsertedDevice", "enabled": True, "parameters": []})()); track.delete_device = lambda index: track.devices.pop(index); mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        with self.assertRaisesRegex(ValueError, "exact requested"): mapper.invoke("device.insert", {"trackRef": row["ref"], "deviceName": "Wrong index", "index": 0, "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in row["devices"]]})
        self.assertEqual(len(track.devices), 0)

    def test_a_new_simpler_can_arrive_with_its_sample_and_goes_again_if_the_sample_fails(self):
        def simpler(name, loads=True):
            device = type("InsertedDevice", (), {"name": name, "class_name": "OriginalSimpler", "enabled": True, "parameters": [FakeParameter()], "sample": None})()
            def replace(path):
                if not loads: raise RuntimeError("Live could not read the file")
                device.sample = type("Sample", (), {"file_path": path})()
            device.replace_sample = replace
            return device
        for loads in (True, False):
            song = FakeSong(); track = song.tracks[0]; track.devices = []
            track.insert_device = lambda name, index, loads=loads: track.devices.append(simpler(name, loads)); track.delete_device = lambda index: track.devices.pop(index)
            mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
            args = {"trackRef": row["ref"], "deviceName": "Simpler", "samplePath": "/Samples/Kick 01.wav", "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": []}
            if loads:
                created = mapper.invoke("device.insert", args)
                self.assertEqual(created["samplePath"], "/Samples/Kick 01.wav"); self.assertEqual(track.devices[0].sample.file_path, "/Samples/Kick 01.wav")
                self.assertEqual(created["createdFingerprint"], mapper._mapped_fingerprint(created["ref"]), "the fingerprint includes the sample, so undo takes both")
            else:
                with self.assertRaisesRegex(RuntimeError, "could not read"): mapper.invoke("device.insert", args)
                self.assertEqual(track.devices, [], "a sample that doesn't load takes the new device away again")
        song = FakeSong(); track = song.tracks[0]; track.devices = []; track.insert_device = lambda name, index: None; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        with self.assertRaisesRegex(ValueError, "absolute path"): mapper.invoke("device.insert", {"trackRef": row["ref"], "deviceName": "Simpler", "samplePath": "Samples/Kick.wav", "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": []})

    def test_cleanup_token_attachment_failure_removes_physical_creation_and_registry_mapping(self):
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live"); track_ref = mapper.snapshot()["tracks"][0]["ref"]; checkpoint = mapper.refs.checkpoint(); mapper._attach_cleanup_ownership = lambda *_args: (_ for _ in ()).throw(RuntimeError("injected token attachment failure"))
        with self.assertRaisesRegex(RuntimeError, "token attachment failure"): mapper.invoke("clip.create", self.clip_creation_args(mapper, track_ref, 0, kind="midi", name="Unattached", length=4), "attachment-failure-transaction")
        self.assertIsNone(song.tracks[0].clip_slots[0].clip); self.assertEqual(mapper.refs.checkpoint(), checkpoint); self.assertEqual(mapper._owned_cleanup_tokens, {})

    def test_mapper_rejects_unsafe_clip_and_note_mutations(self):
        mapper = LiveObjectMapper(FakeSong())
        track = mapper.discover("track")["items"][0]["ref"]
        with self.assertRaises(ValueError):
            mapper.invoke("clip.create", self.clip_creation_args(mapper, track, 0, kind="midi", name="bad", length=float("nan")))
        created = mapper.invoke("clip.create", self.clip_creation_args(mapper, track, 0, kind="midi", name="bounded", length=4))
        with self.assertRaises(ValueError):
            mapper.invoke("note.add", {"ref": created["ref"], "note": {"pitch": 36, "start": 3.5, "duration": 1, "velocity": 100, "channel": 1}, **self.note_authority(mapper, created["ref"])})
        with self.assertRaises(ValueError):
            mapper.invoke("note.add-batch", {"ref": created["ref"], "notes": [
                {"pitch": 36, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1},
                {"pitch": 38, "start": 3.5, "duration": 1, "velocity": 100, "channel": 1},
            ], **self.note_authority(mapper, created["ref"])})
        self.assertEqual(mapper.refs.get(created["ref"]).get_notes(0, 0, 0, 128), [])

    def test_discovery_pages_notes_and_rejects_stale_cursor(self):
        mapper = LiveObjectMapper(FakeSong())
        track = mapper.discover("track")["items"][0]["ref"]
        created = mapper.invoke("clip.create", self.clip_creation_args(mapper, track, 0, kind="midi", name="Paged", length=16))
        mapper.invoke("note.add", {"ref": created["ref"], "note": {"pitch": 36, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1}, **self.note_authority(mapper, created["ref"])})
        page = mapper.discover("note", 1, parent=created["ref"])
        self.assertEqual(len(page["items"]), 1)
        mapper.invoke("session.reconnect", {})
        with self.assertRaises(ValueError):
            mapper.discover("note", 1, page.get("nextCursor", "invalid"), parent=created["ref"])

    def test_timed_out_main_thread_callback_is_fenced_from_late_drain(self):
        work = _MainThreadQueue()
        mutations = []
        errors = []
        cancellations = []
        import threading
        worker = threading.Thread(target=lambda: self._capture_queue_error(work, mutations, errors, cancellations))
        worker.start(); worker.join(1)
        self.assertEqual(errors, ["Live main-thread operation timed out before dispatch"])
        self.assertEqual(cancellations, ["Live main-thread operation timed out before dispatch"])
        self.assertEqual(work.drain(), 1)
        self.assertEqual(cancellations, ["Live main-thread operation timed out before dispatch"])
        self.assertEqual(mutations, [])

    @staticmethod
    def _capture_queue_error(work, mutations, errors, cancellations):
        try: work.submit(lambda: mutations.append("mutated"), timeout=0.01, on_cancel=lambda error: cancellations.append(str(error)))
        except TimeoutError as error: errors.append(str(error))

    def test_nonblocking_main_thread_callback_reports_predispatch_expiry(self):
        work = _MainThreadQueue()
        mutations = []
        cancellations = []
        self.assertTrue(work.submit_nowait(lambda: mutations.append("mutated"), int(time.time() * 1000) + 10, lambda error: cancellations.append(str(error))))
        time.sleep(0.02)
        self.assertEqual(work.drain(), 1)
        self.assertEqual(mutations, [])
        self.assertEqual(cancellations, ["Live main-thread operation timed out before dispatch"])

    def test_bridge_lifecycle_and_main_thread_queue_cleanup(self):
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": 45678, "secret": "0123456789abcdef0123456789abcdef"})
        self.assertGreater(bridge.address[1], 0)
        result = []

        def submit():
            result.append(bridge._dispatch("status", {}))

        import threading
        worker = threading.Thread(target=submit)
        worker.start()
        self.assertEqual(bridge.drain_main_thread(), 1)
        worker.join(1)
        self.assertTrue(result[0]["connected"])
        queued = []
        worker = threading.Thread(target=lambda: queued.append(bridge._dispatch("status", {})))
        worker.start()
        bridge.update_display()
        worker.join(1)
        self.assertEqual(len(queued), 1)
        bridge.disconnect()
        self.assertTrue(bridge._stop.is_set())
        self.assertEqual(len(bridge._clients), 0)

    def test_bridge_accept_polls_without_blocking_and_traces_hard_errors(self):
        # Connections are accepted on Live's main thread without blocking: "nothing pending" is quiet,
        # and a real accept failure is traced for diagnostics instead of raising into Live.
        class FailingServer:
            def __init__(self): self.calls = 0
            def accept(self):
                self.calls += 1
                if self.calls == 1: raise BlockingIOError()
                raise OSError("injected persistent accept failure")

        bridge = object.__new__(AbletonMcpBridge)
        bridge._server = FailingServer(); bridge._stop = threading.Event(); bridge._connections = []; bridge._clients = set()
        with patch("ableton_mcp_remote_script._debug_trace") as trace:
            bridge._accept_pending()
            trace.assert_not_called()
            bridge._accept_pending()
        self.assertEqual(bridge._server.calls, 2)
        trace.assert_called_once_with("bridge-accept-failure")
        self.assertIn("bridge-accept-failure", remote_module._DIAGNOSTIC_EVENTS)

    def test_disconnect_releases_waiting_main_thread_work(self):
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": 45679, "secret": "0123456789abcdef0123456789abcdef"})
        result = []
        import threading
        worker = threading.Thread(target=lambda: result.append(self._dispatch_error(bridge)))
        worker.start()
        bridge.disconnect()
        worker.join(1)
        self.assertEqual(result, ["Live bridge is disconnected"])

    @staticmethod
    def _dispatch_error(bridge):
        try:
            bridge._dispatch("status", {})
        except RuntimeError as error:
            return str(error)
        return "no error"


    def test_audition_refuses_same_slot_clip_substitution_at_live_thread_boundary(self):
        song = FakeAuditionSong(); mapper = LiveObjectMapper(song); _, args = self._audition_args(mapper)
        song.tracks[0].clip_slots[0].clip = FakeClip(4.0)
        with self.assertRaisesRegex(ValueError, "identity hierarchy"):
            mapper.invoke("session.audition-launch", args)
        self.assertFalse(song.is_playing)

    def test_atomic_session_clip_move_compensates_when_source_delete_fails(self):
        song = FakeSong(); song.scenes.append(FakeScene("Scene 2")); source = song.tracks[0].clip_slots[0]; source.clip = FakeClip(4.0); source.clip.name = "Source"; target = FakeSlot(); song.tracks[0].clip_slots.append(target)
        def duplicate(destination): duplicate_clip = FakeClip(source.clip.length); duplicate_clip.name = source.clip.name; destination.clip = duplicate_clip
        source.duplicate_clip_to = duplicate
        def refuse_delete(): raise RuntimeError("injected delete failure")
        source.delete_clip = refuse_delete
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); track = snapshot["tracks"][0]; source_row = track["clips"][0]; target_slot = track["clipSlots"][1]; target_scene = snapshot["scenes"][1]
        args = {"ref": source_row["ref"], "targetTrackRef": track["ref"], "targetSceneIndex": 1, "arrangementPosition": None, **mapper._session_clip_authority(source_row["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(source_row["ref"]), "expectedTargetTrackIdentity": track["objectIdentity"], "expectedTargetSlotRef": target_slot["ref"], "expectedTargetSlotIdentity": target_slot["objectIdentity"], "expectedTargetSceneRef": target_scene["ref"], "expectedTargetSceneIdentity": target_scene["objectIdentity"], "expectedTargetCollectionRevision": None}
        source.clip.name = "External edit"
        with self.assertRaisesRegex(ValueError, "content changed"): mapper.invoke("clip.move", args)
        source.clip.name = "Source"
        with self.assertRaisesRegex(ValueError, "source deletion failed"):
            mapper.invoke("clip.move", args)
        self.assertIsNotNone(source.clip); self.assertIsNone(target.clip)
        source.duplicate_clip_to = lambda destination: setattr(destination, "clip", FakeClip(99.0)); deleted = []; source.delete_clip = lambda: deleted.append(True)
        with self.assertRaisesRegex(ValueError, "preserve exact clip content"): mapper.invoke("clip.move", args)
        self.assertEqual(deleted, []); self.assertIsNotNone(source.clip); self.assertIsNone(target.clip)

    def test_preexisting_clip_move_never_mints_cleanup_authority(self):
        song = FakeSong(); song.scenes.append(FakeScene("Scene 2")); source = song.tracks[0].clip_slots[0]; source.clip = FakeClip(4.0); target = FakeSlot(); song.tracks[0].clip_slots.append(target)
        source.duplicate_clip_to = lambda destination: setattr(destination, "clip", FakeClip(source.clip.length)); mapper = LiveObjectMapper(song, provenance="real-live"); snapshot = mapper.snapshot(); track = snapshot["tracks"][0]; source_row = track["clips"][0]; target_slot = track["clipSlots"][1]; target_scene = snapshot["scenes"][1]; args = {"ref": source_row["ref"], "targetTrackRef": track["ref"], "targetSceneIndex": 1, "arrangementPosition": None, **mapper._session_clip_authority(source_row["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(source_row["ref"]), "expectedTargetTrackIdentity": track["objectIdentity"], "expectedTargetSlotRef": target_slot["ref"], "expectedTargetSlotIdentity": target_slot["objectIdentity"], "expectedTargetSceneRef": target_scene["ref"], "expectedTargetSceneIdentity": target_scene["objectIdentity"], "expectedTargetCollectionRevision": None}; moved = mapper.invoke("clip.move", args, "preexisting-move-transaction")
        self.assertNotIn("ownershipToken", moved); self.assertEqual(mapper._owned_cleanup_tokens, {}); self.assertIsNone(source.clip); self.assertIsNotNone(target.clip)

    def test_capture_midi_refuses_any_preexisting_session_content(self):
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = FakeClip(4.0); called = []; song.capture_midi = lambda: called.append(True); mapper = LiveObjectMapper(song); expected = mapper._capture_authority_revision()
        # Advertised on shape (Live can capture MIDI; the Set isn't walked to see whether its slots are empty):
        # the capture itself refuses pre-existing content.
        self.assertIn("session.capture-midi", mapper.status()["operations"])
        with self.assertRaisesRegex(ValueError, "globally empty Session slots"): mapper.invoke("session.capture-midi", {"expectedStateRevision": expected})
        self.assertEqual(called, []); self.assertIsNotNone(song.tracks[0].clip_slots[0].clip)

    def test_partial_note_delete_restores_complete_content_with_fresh_stable_id(self):
        class Note:
            def __init__(self, note_id, pitch): self.note_id = note_id; self.pitch = pitch; self.start_time = 0.0; self.duration = 0.25; self.velocity = 100; self.channel = 1; self.mute = False; self.probability = 1.0; self.velocity_deviation = 0.0; self.release_velocity = 64.0
        class Clip:
            length = 4.0
            def __init__(self): self.notes = [Note(1, 36), Note(2, 38)]; self.next_id = 3
            def get_all_notes_extended(self): return list(self.notes)
            def remove_notes_by_id(self, ids): self.notes = [note for note in self.notes if note.note_id != ids[0]]; raise RuntimeError("injected partial delete")
            def add_new_notes(self, notes): self.notes.append(Note(self.next_id, notes[0]["pitch"])); self.next_id += 1
        song = FakeSong(); clip = Clip(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        with self.assertRaisesRegex(RuntimeError, "partial delete"): mapper.invoke("note.delete", {"ref": ref, "noteIds": [1, 2], **self.note_authority(mapper, ref)})
        self.assertEqual(sorted(note.pitch for note in clip.notes), [36, 38]); self.assertEqual(len({note.note_id for note in clip.notes}), 2)

    def test_clip_delete_requires_authoritative_absence(self):
        song = FakeSong(); slot = song.tracks[0].clip_slots[0]; slot.clip = FakeClip(4.0); slot.delete_clip = lambda: None; mapper = LiveObjectMapper(song); ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "not confirmed"): mapper.invoke("clip.delete", {"ref": ref, **mapper._session_clip_authority(ref)})
        self.assertIsNotNone(slot.clip); self.assertIs(mapper.refs.get(ref), slot.clip)

    def test_device_enable_setter_failure_restores_prior_state(self):
        class FailingDevice:
            name = "Failing"; class_name = "Failing"; parameters = []
            def __init__(self): self._enabled = False
            @property
            def enabled(self): return self._enabled
            @enabled.setter
            def enabled(self, value): self._enabled = value; raise RuntimeError("injected setter acknowledgement loss")
        song = FakeSong(); device = FailingDevice(); song.tracks[0].devices = [device]; mapper = LiveObjectMapper(song); track = mapper.snapshot()["tracks"][0]; row = track["devices"][0]; siblings = [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in track["devices"]]; args = {"ref": row["ref"], "enabled": True, "expectedObjectIdentity": row["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"enabled": False}).encode()).hexdigest()}
        with self.assertRaisesRegex(ValueError, "unavailable"): mapper.invoke("device.enable", args)
        self.assertFalse(device.enabled)
        class AliasDevice:
            name = "Alias"; class_name = "Alias"; parameters = []
            def __init__(self): self.enabled = False
            @property
            def is_active(self): return False
            @is_active.setter
            def is_active(self, _value): raise RuntimeError("read-only authoritative alias")
        song = FakeSong(); alias = AliasDevice(); song.tracks[0].devices = [alias]; mapper = LiveObjectMapper(song); track = mapper.snapshot()["tracks"][0]; row = track["devices"][0]; siblings = [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in track["devices"]]; args = {"ref": row["ref"], "enabled": True, "expectedObjectIdentity": row["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"enabled": False}).encode()).hexdigest()}
        with self.assertRaisesRegex(ValueError, "unavailable"): mapper.invoke("device.enable", args)
        self.assertFalse(alias.is_active); self.assertFalse(alias.enabled)

    def test_device_enable_device_on_parameter_failure_rolls_back_exactly(self):
        class FailingOnParameter(FakeParameter):
            def __init__(self): self._armed = False; super().__init__(); self.quantization = 1.0; self._value = 1.0
            @property
            def value(self): return self._value
            @value.setter
            def value(self, target):
                self._value = target
                if self._armed: raise RuntimeError("injected setter acknowledgement loss")
        class ToggleDevice(FakeDevice):
            def __init__(self):
                super().__init__(); on = FailingOnParameter(); on._armed = True; self.parameters = [on, FakeParameter()]
            @property
            def enabled(self): return self.parameters[0].value == 1.0
            @enabled.setter
            def enabled(self, _value): pass  # enable state is owned by the Device On parameter
        song = FakeSong(); device = ToggleDevice(); song.tracks[0].devices = [device]; mapper = LiveObjectMapper(song); track = mapper.snapshot()["tracks"][0]; row = track["devices"][0]; siblings = [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in track["devices"]]; args = {"ref": row["ref"], "enabled": False, "expectedObjectIdentity": row["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"enabled": True}).encode()).hexdigest()}
        with self.assertRaisesRegex(ValueError, "unavailable"): mapper.invoke("device.enable", args)
        self.assertTrue(device.enabled)

    def test_browser_search_skips_unrepresentable_live_names(self):
        class Item:
            def __init__(self, name, children=None): self.name = name; self.children = children or []; self.is_loadable = not bool(children); self.is_device = not bool(children)
        class Browser:
            instruments = Item("instruments", [Item("x" * 300), Item("Bounded")])
        mapper = LiveObjectMapper(FakeSong()); mapper._browser = lambda: Browser(); result = mapper.invoke("browser.search", {"category": "instruments", "limit": 10})
        self.assertEqual([item["name"] for item in result["items"]], ["Bounded"]); validate_operation_payload("browser.search", "result", result)
        class BroadBrowser:
            instruments = Item("instruments", [Item(f"Item {index}") for index in range(257)])
        mapper._browser = lambda: BroadBrowser()
        with patch.object(remote_module, "MAX_DISCOVERY_COLLECTION_LENGTH", 256), self.assertRaisesRegex(ValueError, "traversal bound"): mapper.invoke("browser.search", {"category": "instruments", "query": "never-matches", "limit": 10})

    def test_browser_inspect_follows_the_returned_path_without_scanning_unrelated_subtrees(self):
        class Item:
            def __init__(self, name, children=None): self.name = name; self.children = children or []; self.is_loadable = not bool(children); self.is_device = not bool(children)
        class Browser:
            @property
            def instruments(self):
                return Item("instruments", [
                    Item("Before", [Item(f"Before {index}") for index in range(200)]),
                    Item("Target Folder", [Item("Operator")]),
                    Item("After", [Item(f"After {index}") for index in range(100)]),
                ])
        mapper = LiveObjectMapper(FakeSong()); mapper._browser = lambda: Browser()
        item = mapper.invoke("browser.search", {"category": "instruments", "query": "Operator", "limit": 1})["items"][0]
        self.assertEqual(mapper.invoke("browser.inspect", {"itemId": item["id"]}), item)

    def test_arrangement_duplicate_identifies_new_clip_and_move_compensates(self):
        song = FakeSong(); track = song.tracks[0]; source_slot = track.clip_slots[0]; source_slot.clip = FakeClip(4.0); source_slot.clip.name = "Session Source"
        existing = FakeClip(4.0); existing.name = "Existing at Eight"; existing.start_time = 8.0; track.arrangement_clips = [existing]
        def duplicate_to_arrangement(clip, position): created = FakeClip(clip.length); created.name = clip.name; created.start_time = position; track.arrangement_clips.append(created)
        track.duplicate_clip_to_arrangement = duplicate_to_arrangement
        def delete_clip(candidate): track.arrangement_clips.remove(candidate)
        track.delete_clip = delete_clip
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); source = snapshot["tracks"][0]["clips"][0]
        args = {"ref": source["ref"], "targetTrackRef": None, "targetSceneIndex": None, "arrangementPosition": 8.0, **mapper._session_clip_authority(source["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(source["ref"]), "expectedTargetTrackIdentity": None, "expectedTargetSlotRef": None, "expectedTargetSlotIdentity": None, "expectedTargetSceneRef": None, "expectedTargetSceneIdentity": None, "expectedTargetCollectionRevision": mapper._arrangement_collection_revision(track, 0)}
        created = mapper.invoke("clip.duplicate", args); self.assertNotEqual(created["objectIdentity"], mapper.snapshot()["arrangement"]["clips"][0]["objectIdentity"]); self.assertEqual(created["createdFingerprint"], mapper._mapped_fingerprint(created["ref"]))
        source_arrangement = mapper.snapshot()["arrangement"]["clips"][0]; source_object = track.arrangement_clips[0]
        def selective_delete(candidate):
            if candidate is not source_object: track.arrangement_clips.remove(candidate)
        track.delete_clip = selective_delete
        before = len(track.arrangement_clips); move_args = {"ref": source_arrangement["ref"], "position": 16.0, "expectedObjectIdentity": source_arrangement["objectIdentity"], "expectedAuthorityRevision": mapper._arrangement_clip_authority_revision(source_arrangement["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(source_arrangement["ref"])}
        with self.assertRaisesRegex(ValueError, "source deletion failed"):
            mapper.invoke("arrangement.clip.move", move_args)
        self.assertEqual(len(track.arrangement_clips), before); self.assertIn(source_object, track.arrangement_clips)

    def test_arrangement_move_fingerprint_is_the_clip_content_not_its_playback(self):
        # The host fingerprints clips without playback state; a moved clip that plays must still match.
        song = FakeSong(); track = song.tracks[0]
        clip = FakeClip(4.0); clip.name = "Moving"; clip.start_time = 8.0; clip.playing_position = 1.5; clip.is_playing = True; track.arrangement_clips = [clip]
        def duplicate_to_arrangement(source, position):
            created = FakeClip(source.length); created.name = source.name; created.start_time = position; created.playing_position = 3.25; created.is_playing = True
            track.arrangement_clips.append(created)
        track.duplicate_clip_to_arrangement = duplicate_to_arrangement
        track.delete_clip = lambda candidate: track.arrangement_clips.remove(candidate)
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["arrangement"]["clips"][0]
        moved = mapper.invoke("arrangement.clip.move", {"ref": row["ref"], "position": 16.0, "expectedObjectIdentity": row["objectIdentity"],
            "expectedAuthorityRevision": mapper._arrangement_clip_authority_revision(row["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])})
        self.assertEqual(moved["createdFingerprint"], mapper._mapped_fingerprint(moved["ref"]))
        track.arrangement_clips[0].playing_position = 0.5
        self.assertEqual(moved["createdFingerprint"], mapper._mapped_fingerprint(moved["ref"]), "playback moving on doesn't change the moved clip's fingerprint")

    @staticmethod
    def arrangement_track_that_crashes_on_overlap(*clips):
        """A track whose Arrangement copy fails where Live crashes: onto a span a clip already holds."""
        song = FakeSong(); track = song.tracks[0]; track.arrangement_clips = []; track.copies = []
        for name, start, length in clips:
            clip = FakeClip(length); clip.name = name; clip.start_time = start; clip.end_time = start + length; clip.add_new_notes([{"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100}]); track.arrangement_clips.append(clip)
        def duplicate_to_arrangement(source, position):
            span = source.end_time - source.start_time
            if any(other.start_time < position + span and other.end_time > position for other in track.arrangement_clips): raise RuntimeError("Live crashed: an Arrangement clip copied onto a clip")
            created = FakeClip(source.length); created.name = source.name; created.start_time = position; created.end_time = position + span; created.notes = [dict(note) for note in source.notes]
            track.copies.append(position); track.arrangement_clips.append(created); track.arrangement_clips.sort(key=lambda clip: clip.start_time)
        track.duplicate_clip_to_arrangement = duplicate_to_arrangement; track.delete_clip = lambda candidate: track.arrangement_clips.remove(candidate)
        return song, track

    @staticmethod
    def arrangement_move(mapper, row, position):
        return mapper.invoke("arrangement.clip.move", {"ref": row["ref"], "position": position, "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._arrangement_clip_authority_revision(row["ref"]), "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])})

    def test_a_move_by_less_than_the_clips_length_parks_it_past_the_end_of_the_set_first(self):
        song, track = self.arrangement_track_that_crashes_on_overlap(("Loop", 4.0, 8.0)); song.song_length = 64.0
        mapper = LiveObjectMapper(song); moved = self.arrangement_move(mapper, mapper.snapshot()["arrangement"]["clips"][0], 6.0)
        self.assertEqual(track.copies, [68.0, 6.0], "parked past the end of the Set, then copied into place")
        self.assertEqual([(clip.name, clip.start_time, len(clip.notes)) for clip in track.arrangement_clips], [("Loop", 6.0, 1)])
        self.assertEqual(moved["start"], 6.0); self.assertEqual(moved["createdFingerprint"], mapper._mapped_fingerprint(moved["ref"]))
        back = self.arrangement_move(mapper, mapper.snapshot()["arrangement"]["clips"][0], 4.0)
        self.assertEqual((track.copies[2:], back["start"], [clip.start_time for clip in track.arrangement_clips]), ([68.0, 4.0], 4.0, [4.0]), "undo moves it back the same way")

    def test_a_move_onto_another_clip_is_refused_before_anything_is_copied(self):
        song, track = self.arrangement_track_that_crashes_on_overlap(("Verse", 0.0, 4.0), ("Chorus", 8.0, 4.0))
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["arrangement"]["clips"][0]
        with self.assertRaisesRegex(ValueError, r"^Kumi can't move a clip onto another clip yet: \"Chorus\" \(beats 8 to 12\) is in the way at beat 6; clear that span first \(clear_range\) or pick a free spot; nothing changed$"): self.arrangement_move(mapper, row, 6.0)
        self.assertEqual((track.copies, [(clip.name, clip.start_time) for clip in track.arrangement_clips]), ([], [("Verse", 0.0), ("Chorus", 8.0)]))
        self.assertEqual(self.arrangement_move(mapper, row, 4.0)["start"], 4.0, "right up against it is fine")

    def test_a_parked_clip_that_cant_be_copied_into_place_goes_back_where_it_was(self):
        song, track = self.arrangement_track_that_crashes_on_overlap(("Loop", 4.0, 8.0)); copy = track.duplicate_clip_to_arrangement
        def refuse_the_target(source, position):
            if position == 6.0: raise RuntimeError("injected copy failure")
            copy(source, position)
        track.duplicate_clip_to_arrangement = refuse_the_target
        mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(ValueError, "back where it was, as a new clip"): self.arrangement_move(mapper, mapper.snapshot()["arrangement"]["clips"][0], 6.0)
        self.assertEqual((track.copies, [(clip.name, clip.start_time) for clip in track.arrangement_clips]), ([18.0, 4.0], [("Loop", 4.0)]))

    def test_transport_revision_rejects_observed_aba_state(self):
        song = FakeSong()
        song.loop = False
        song.loop_start = 0.0
        song.loop_length = 4.0
        song.current_song_time = 0.0
        song.metronome = False
        song.punch_in = False
        song.punch_out = False
        song.count_in_duration = 0
        mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        set_row = snapshot["set"]

        def set_metronome(value, revision):
            return mapper.invoke("transport.set", {
                "setRef": set_row["ref"],
                "expectedObjectIdentity": set_row["objectIdentity"],
                "expectedRevision": revision,
                "metronome": value,
            })

        initial_revision = snapshot["playback"]["revision"]
        first_true = set_metronome(True, initial_revision)["revision"]
        intervening_false = set_metronome(False, first_true)["revision"]
        current_true = set_metronome(True, intervening_false)["revision"]
        self.assertNotEqual(first_true, current_true)
        self.assertEqual(current_true, mapper.snapshot()["playback"]["revision"])
        with self.assertRaisesRegex(ValueError, "changed since preview"):
            set_metronome(False, first_true)
        self.assertTrue(song.metronome)
        set_metronome(False, current_true)

    def test_device_delete_takes_one_device_from_among_others(self):
        song = FakeSong(); first, second, third = FakeDevice(), FakeDevice(), FakeDevice(); first.name, second.name, third.name = "Operator", "Reverb", "Utility"
        song.tracks[0].devices = [first, second, third]; song.tracks[0].delete_device = lambda index: song.tracks[0].devices.pop(index); mapper = LiveObjectMapper(song)
        track = mapper.snapshot()["tracks"][0]; device = track["devices"][1]; siblings = [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in track["devices"]]
        mapper.invoke("device.delete", {"ref": device["ref"], "expectedObjectIdentity": device["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"]})
        self.assertEqual([device.name for device in song.tracks[0].devices], ["Operator", "Utility"])

    def test_wrong_device_delete_and_late_transport_failure_never_report_partial_success(self):
        song = FakeSong(); target, sibling = FakeDevice(), FakeDevice(); target.name = "Target"; sibling.name = "Sibling"; song.tracks[0].devices = [target, sibling]; song.tracks[0].delete_device = lambda _index: song.tracks[0].devices.pop(1); mapper = LiveObjectMapper(song); track = mapper.snapshot()["tracks"][0]; device = track["devices"][0]; siblings = [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in track["devices"]]
        # Live deleting another device than the one asked for is reported, never taken as done.
        with self.assertRaisesRegex(ValueError, "did not preserve the exact authorized siblings"): mapper.invoke("device.delete", {"ref": device["ref"], "expectedObjectIdentity": device["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"]})
        self.assertIn(target, song.tracks[0].devices)
        class FailingTransportSong(FakeSong):
            def __init__(self): self._loop = False; self.reject_loop = False; super().__init__(); self.reject_loop = True
            @property
            def loop(self): return self._loop
            @loop.setter
            def loop(self, value): self._loop = value
        failing = FailingTransportSong(); failing.loop_start = 0.0; failing.loop_length = 4.0; failing.current_song_time = 0.0; failing.metronome = False; failing.punch_in = False; failing.punch_out = False
        def reject_loop(value): failing._loop = value; raise RuntimeError("injected late transport failure")
        type(failing).loop = property(lambda self: self._loop, lambda self, value: reject_loop(value) if self.reject_loop else setattr(self, "_loop", value)); mapper = LiveObjectMapper(failing); snapshot = mapper.snapshot(); set_ref = snapshot["set"]["ref"]
        with self.assertRaisesRegex(RuntimeError, "late transport failure"): mapper.invoke("transport.set", {"setRef": set_ref, "expectedObjectIdentity": snapshot["set"]["objectIdentity"], "expectedRevision": snapshot["playback"]["revision"], "loopEnabled": True, "metronome": True})
        self.assertIs(failing.loop, False); self.assertFalse(failing.metronome)

    def test_mixer_and_routing_mutations_compare_atomic_prior_state(self):
        song = FakeSong(); track = song.tracks[0]; track.mute = False; track.solo = False
        volume, pan, cue, send = FakeParameter(), FakeParameter(), FakeParameter(), FakeParameter(); volume.value = 0.5; pan.value = 0.0; cue.value = 0.7; send.value = 0.1
        track.mixer_device = type("Mixer", (), {"volume": volume, "panning": pan, "cue_volume": cue, "sends": [send]})()
        track.available_input_routing_types = [FakeRouteChoice("Ext. In")]; track.available_input_routing_channels = [FakeRouteChoice("1")]; track.available_output_routing_types = [FakeRouteChoice("Main")]; track.available_output_routing_channels = [FakeRouteChoice("1")]; track.input_routing_type = track.available_input_routing_types[0]; track.input_routing_channel = track.available_input_routing_channels[0]; track.output_routing_type = track.available_output_routing_types[0]; track.output_routing_channel = track.available_output_routing_channels[0]; track.can_be_armed = True
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; mixer = row["mixer"]; state = {field: mixer.get(field) for field in ("volume", "pan", "mute", "solo", "cueVolume", "sends")}
        args = {"ref": row["ref"], "volume": 0.8, "expectedObjectIdentity": row["objectIdentity"], "expectedVolumeIdentity": mixer["volumeIdentity"], "expectedPanIdentity": mixer["panIdentity"], "expectedCueIdentity": mixer["cueIdentity"], "expectedSendIdentities": mixer["sendIdentities"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        volume.value = 0.6
        with self.assertRaisesRegex(ValueError, "state changed"):
            mapper.invoke("mixer.set", args)
        self.assertEqual(volume.value, 0.6)
        row = mapper.snapshot()["tracks"][0]; routing = row["routing"]; routing_state = {"inputType": routing["inputType"], "inputSubRouting": routing["inputSubRouting"], "outputType": routing["outputType"], "outputSubRouting": routing["outputSubRouting"], "arm": row["armed"], "monitoring": row["monitoringState"]}; routing_args = {"ref": row["ref"], "outputType": "Main", "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(routing_state).encode()).hexdigest()}
        track.arm = True
        with self.assertRaisesRegex(ValueError, "state changed"):
            mapper.invoke("routing.set", routing_args)

    def test_a_routing_live_no_longer_offers_is_refused_with_nothing_changed(self):
        song = FakeSong(); track = song.tracks[0]
        track.available_input_routing_types = [FakeRouteChoice("Ext. In")]; track.available_input_routing_channels = [FakeRouteChoice("1")]; track.available_output_routing_types = [FakeRouteChoice("Main")]; track.available_output_routing_channels = [FakeRouteChoice("1")]; track.input_routing_type = track.available_input_routing_types[0]; track.input_routing_channel = track.available_input_routing_channels[0]; track.output_routing_type = track.available_output_routing_types[0]; track.output_routing_channel = track.available_output_routing_channels[0]; track.can_be_armed = True
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; routing = row["routing"]
        state = {"inputType": routing["inputType"], "inputSubRouting": routing["inputSubRouting"], "outputType": routing["outputType"], "outputSubRouting": routing["outputSubRouting"], "arm": row["armed"], "monitoring": row["monitoringState"]}
        args = {"ref": row["ref"], "inputType": "Kumi Pad", "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        with self.assertRaisesRegex(ValueError, "^Live doesn't offer Kumi Pad for this track now; nothing changed$"): mapper.invoke("routing.set", args)
        self.assertEqual(mapper.snapshot()["tracks"][0]["routing"]["inputType"], "Ext. In")

    def test_mixer_multi_field_failure_rolls_back_exact_prior_state(self):
        class FailingPan(FakeParameter):
            def __init__(self): self._value = 0.0; self.reject = False; super().__init__(); self._value = 0.0; self.reject = True
            @property
            def value(self): return self._value
            @value.setter
            def value(self, value):
                if getattr(self, "reject", False) and value == 0.25: raise RuntimeError("injected pan failure")
                self._value = value
        song = FakeSong(); track = song.tracks[0]; volume = FakeParameter(); volume.value = 0.5; pan = FailingPan(); cue = FakeParameter(); cue.value = 0.7; track.mute = False; track.solo = False; track.mixer_device = type("Mixer", (), {"volume": volume, "panning": pan, "cue_volume": cue, "sends": []})()
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; mixer = row["mixer"]; state = {field: mixer.get(field) for field in ("volume", "pan", "mute", "solo", "cueVolume", "sends")}; args = {"ref": row["ref"], "volume": 0.75, "pan": 0.25, "expectedObjectIdentity": row["objectIdentity"], "expectedVolumeIdentity": mixer["volumeIdentity"], "expectedPanIdentity": mixer["panIdentity"], "expectedCueIdentity": mixer["cueIdentity"], "expectedSendIdentities": mixer["sendIdentities"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        with self.assertRaisesRegex(RuntimeError, "injected pan failure"): mapper.invoke("mixer.set", args)
        self.assertEqual(volume.value, 0.5); self.assertEqual(pan.value, 0.0)

    def test_routing_chooses_cycle_free_transition_order_and_rolls_back_failure(self):
        class OrderedTrack(FakeTrack):
            def __init__(self): self.route_log = []; self.reject_input_b = False; self.reject_arm = False; self.input_channels_by_type = {}; self.output_channels_by_type = {}; self.input_types_by_output = {}; super().__init__()
            def __setattr__(self, name, value):
                if name == "arm" and getattr(self, "reject_arm", False) and value is True: raise RuntimeError("injected arm failure")
                if name in {"input_routing_type", "output_routing_type"} and hasattr(self, "route_log"):
                    self.route_log.append((name, getattr(value, "name", None)))
                    if name == "input_routing_type" and getattr(self, "reject_input_b", False) and getattr(value, "name", None) == "B": raise RuntimeError("injected routing failure")
                    super().__setattr__(name, value); direction = "input" if name.startswith("input") else "output"; choices = getattr(self, f"{direction}_channels_by_type", {}).get(getattr(value, "name", None))
                    if choices: super().__setattr__(f"available_{direction}_routing_channels", choices); super().__setattr__(f"{direction}_routing_channel", choices[0])
                    if direction == "output":
                        types = getattr(self, "input_types_by_output", {}).get(getattr(value, "name", None))
                        if types: super().__setattr__("available_input_routing_types", types)
                    return
                super().__setattr__(name, value)
        song = FakeSong(); a = OrderedTrack(); b = FakeTrack(); a.name = "A"; b.name = "B"; song.tracks = [a, b]
        ext, route_b, main = FakeRouteChoice("Ext. In"), FakeRouteChoice("B"), FakeRouteChoice("Main"); route_a = FakeRouteChoice("A"); channel = FakeRouteChoice("1"); channel_two = FakeRouteChoice("2")
        for track in song.tracks:
            track.available_input_routing_types = [ext, route_a, route_b]; track.available_input_routing_channels = [channel, channel_two]; track.available_output_routing_types = [route_a, route_b, main]; track.available_output_routing_channels = [channel, channel_two]; track.input_routing_channel = channel; track.output_routing_channel = channel; track.can_be_armed = True
        a.input_channels_by_type = {"Ext. In": [channel], "B": [channel_two]}; a.output_channels_by_type = {"B": [channel], "Main": [channel_two]}; a.input_types_by_output = {"B": [ext, route_a], "Main": [ext, route_a, route_b]}
        a.input_routing_type = ext; a.output_routing_type = route_b; b.input_routing_type = ext; b.output_routing_type = main; a.route_log.clear()
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; routing = row["routing"]; state = {"inputType": routing["inputType"], "inputSubRouting": routing["inputSubRouting"], "outputType": routing["outputType"], "outputSubRouting": routing["outputSubRouting"], "arm": row["armed"], "monitoring": row["monitoringState"]}; args = {"ref": row["ref"], "inputType": "B", "outputType": "Main", "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        self.assertTrue(mapper.invoke("routing.set", args)["changed"]); self.assertEqual(a.route_log[:2], [("output_routing_type", "Main"), ("input_routing_type", "B")])
        a.input_routing_type = ext; a.output_routing_type = route_b; a.input_routing_channel = channel; a.output_routing_channel = channel; a.route_log.clear(); a.reject_arm = True; row = mapper.snapshot()["tracks"][0]; routing = row["routing"]; state = {"inputType": routing["inputType"], "inputSubRouting": routing["inputSubRouting"], "outputType": routing["outputType"], "outputSubRouting": routing["outputSubRouting"], "arm": row["armed"], "monitoring": row["monitoringState"]}; args.update({"inputSubRouting": "2", "outputSubRouting": "2", "arm": True, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()})
        with self.assertRaisesRegex(RuntimeError, "injected arm failure"): mapper.invoke("routing.set", args)
        self.assertEqual(a.output_routing_type.name, "B"); self.assertEqual(a.input_routing_type.name, "Ext. In"); self.assertEqual(a.input_routing_channel.name, "1"); self.assertEqual(a.output_routing_channel.name, "1")


if __name__ == "__main__":
    unittest.main()


class SongResolutionTests(unittest.TestCase):
    def test_bridge_resolves_callable_song_accessor(self):
        class CallableSongInstance:
            def __init__(self):
                self._song = FakeSong()
            def song(self):
                return self._song

        probe = __import__("socket").socket(__import__("socket").AF_INET, __import__("socket").SOCK_STREAM)
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
        probe.close()
        bridge = AbletonMcpBridge(CallableSongInstance(), {"host": "127.0.0.1", "port": port, "secret": "0123456789abcdef0123456789abcdef"})
        try:
            self.assertEqual(bridge.mapper.song.tracks[0].name, "Drums")
        finally:
            bridge.disconnect()

    def test_bridge_uses_direct_song_object_unchanged(self):
        class DirectSongInstance:
            def __init__(self):
                self.song = FakeSong()

        probe = __import__("socket").socket(__import__("socket").AF_INET, __import__("socket").SOCK_STREAM)
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
        probe.close()
        bridge = AbletonMcpBridge(DirectSongInstance(), {"host": "127.0.0.1", "port": port, "secret": "0123456789abcdef0123456789abcdef"})
        try:
            self.assertEqual(bridge.mapper.song.tracks[0].name, "Drums")
        finally:
            bridge.disconnect()


class RealLiveShapeTests(unittest.TestCase):
    def test_snapshot_treats_raising_track_properties_as_unavailable(self):
        class RaisingReturnTrack:
            name = "Return A"
            is_return = True
            clip_slots = []
            devices = []
            @property
            def arm(self):
                raise RuntimeError("Main and Return Tracks have no 'Arm' state!")
            @property
            def current_monitoring_state(self):
                raise RuntimeError("Main and Return Tracks have no monitoring state!")
            @property
            def playing_slot_index(self):
                raise RuntimeError("no slot index")
            @property
            def fired_slot_index(self):
                raise RuntimeError("no slot index")

        class RaisingMasterTrack(RaisingReturnTrack):
            name = "Master"
            is_return = False
            is_master = True

        class SongWithRaisingTracks(FakeSong):
            def __init__(self):
                super().__init__()
                self.return_tracks = [RaisingReturnTrack()]
                self.master_track = RaisingMasterTrack()

        snapshot = LiveObjectMapper(SongWithRaisingTracks()).snapshot()
        self.assertEqual(len(snapshot["tracks"]), 3)
        ret = snapshot["tracks"][1]
        self.assertEqual(ret["kind"], "return")
        self.assertIsNone(ret["armed"])
        self.assertIsNone(ret["monitoringState"])
        self.assertIsNone(ret["playingSlotIndex"])
        main = snapshot["tracks"][2]
        self.assertEqual(main["kind"], "main")


class BoostEnumShapeTests(unittest.TestCase):
    def test_quantization_enum_becomes_plain_int_with_canonical_name(self):
        class FakeBoostQuantization(int):
            def __str__(self):
                return "q_bar"

        class EnumSong(FakeSong):
            def __init__(self):
                super().__init__()
                self.clip_trigger_quantization = FakeBoostQuantization(4)

        playback = LiveObjectMapper(EnumSong()).snapshot()["playback"]
        transport = playback["transport"]
        self.assertEqual(transport["launchQuantization"]["raw"], 4)
        self.assertIs(type(transport["launchQuantization"]["raw"]), int)
        self.assertEqual(transport["launchQuantization"]["normalized"], "1-bar")

    def test_canonical_renders_int_subclass_enums_as_plain_integers(self):
        class FakeBoostQuantization(int):
            def __str__(self):
                return "q_bar"

        canonical = AuthenticatedRemoteScript._bounded_canonical({"raw": FakeBoostQuantization(4)})
        self.assertEqual(canonical, '{"raw":4}')


class RealtimePlaneTests(unittest.TestCase):
    def _plane(self):
        import socket as _socket
        from ableton_mcp_remote_script import _RealtimePlane

        class _Queue:
            def __init__(self):
                self.calls = []
                self.accept = True
                self.defer = False
                self.raise_once = False
            def submit_nowait(self, callback, deadline_ms, on_cancel=None):
                if self.raise_once:
                    self.raise_once = False
                    raise RuntimeError("injected queue failure")
                if not self.accept:
                    return False
                self.calls.append(callback)
                if not self.defer:
                    try:
                        callback()
                    except BaseException:
                        pass
                return True

        class _Parameter:
            def __init__(self):
                self.min = 0.0
                self.max = 1.0
                self.value = 0.0
                self.enabled = True
                self.quantization = 0.0

        class _Mapper:
            def __init__(self):
                self.parameters = {}; self.authority_generation = 1
            def _playback(self):
                return {"firedTargets": [], "playingTargets": []}
            def _active_targets(self, playback):
                return []
            def _target_key(self, target):
                return "t|s|sc"
            def _guarded_emergency_stop(self, args):
                return {"stopped": True, "stoppedTargets": []}
            def _resolve_parameter(self, ref):
                return self.parameters.setdefault(ref, _Parameter())
            def _realtime_parameter_authority(self, ref):
                parameter = self._resolve_parameter(ref)
                return {"ref": ref, "parameterIdentity": f"parameter:{id(parameter)}", "ownerRef": "owner", "ownerIdentity": f"owner:{self.authority_generation}", "trackRef": "track", "trackIdentity": "track:1", "siblings": [{"ref": ref, "objectIdentity": f"parameter:{id(parameter)}"}]}
            @staticmethod
            def _read_attr(obj, *names):
                for name in names:
                    value = getattr(obj, name, None)
                    if value is not None:
                        return value
                return None

        class _Bridge:
            def __init__(self):
                self.queue = _Queue()
                self.mapper = _Mapper()

        probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM)
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
        probe.close()
        return _RealtimePlane(_Bridge(), "127.0.0.1", port)

    def _arm(self, plane, ttl_ms, channels, references, source_ports=None):
        authorities = [plane._bridge.mapper._realtime_parameter_authority(reference) for reference in references]
        return plane.arm(ttl_ms, channels, references, source_ports, authorities)

    @staticmethod
    def _json(**values):
        return json.dumps(values, separators=(",", ":")).encode()

    @staticmethod
    def _osc_string(value):
        encoded = value.encode() + b"\0"
        return encoded + b"\0" * ((-len(encoded)) % 4)

    @classmethod
    def _osc_parameter(cls, token, sequence, reference, value):
        import struct
        return b"".join((
            cls._osc_string("/ableton-mcp/parameter"), cls._osc_string(",sisf"),
            cls._osc_string(token), struct.pack(">i", sequence), cls._osc_string(reference), struct.pack(">f", value),
        ))

    def test_bridge_realtime_port_validation_and_conflict_cleanup(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
        tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        blocker = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM)
        blocker.bind(("127.0.0.1", 0)); realtime_port = blocker.getsockname()[1]
        try:
            with self.assertRaises(ValueError):
                AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": tcp_port, "secret": "x" * 40})
            with self.assertRaises(OSError):
                AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
            checker = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
            checker.setsockopt(_socket.SOL_SOCKET, _socket.SO_REUSEADDR, 1)
            try:
                checker.bind(("127.0.0.1", tcp_port))
            finally:
                checker.close()
        finally:
            blocker.close()

    def test_configured_plane_is_truthfully_capability_negotiated(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
        tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        udp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM)
        udp_probe.bind(("127.0.0.1", 0)); realtime_port = udp_probe.getsockname()[1]; udp_probe.close()
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
        try:
            status = bridge.mapper.status()
            for operation in ("realtime.arm", "realtime.disarm", "realtime.stats"):
                self.assertIn(operation, status["operations"])
            for capability in ("osc", "realtime.events"):
                self.assertIn(capability, status["capabilities"])
            self.assertNotIn("max", status["capabilities"])
        finally:
            bridge.disconnect()

    def test_authenticated_disarm_and_rearm_revoke_fifo_callbacks_before_drain(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
        tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        udp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM)
        udp_probe.bind(("127.0.0.1", 0)); realtime_port = udp_probe.getsockname()[1]; udp_probe.close()
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
        try:
            snapshot = bridge.mapper.snapshot()
            parameter_ref = snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]
            parameter = bridge.mapper._resolve_parameter(parameter_ref); target_authority = bridge.mapper._realtime_parameter_authority(parameter_ref)
            def authorized(request):
                # This test runs on its synthetic main thread; bridge authority
                # preflight/prepare/invoke sequencing is covered separately.
                return bridge._realtime_op(request["operation"], request["args"])
            arm_request = {"operation": "realtime.arm", "args": {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [parameter_ref], "targetAuthorities": [target_authority], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}}}
            with self.assertRaises(ValueError): bridge._realtime_op("realtime.arm", {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [parameter_ref]})
            armed = authorized(arm_request)
            for sequence in (1, 2):
                bridge._realtime._handle(self._json(token=armed["token"], seq=sequence, channel="udp-json", op="parameter.set", ref=parameter_ref, value=0.75))
            self.assertEqual(bridge.queue.items.qsize(), 2)
            self.assertEqual(authorized({"operation": "realtime.disarm", "args": {}}), {"armed": False})

            armed = authorized(arm_request)
            for sequence in (1, 2):
                bridge._realtime._handle(self._json(token=armed["token"], seq=sequence, channel="udp-json", op="parameter.set", ref=parameter_ref, value=1.0))
            self.assertEqual(bridge.queue.items.qsize(), 4)
            authorized(arm_request)
            self.assertEqual(bridge.queue.drain(), 4)
            self.assertEqual(parameter.value, 0.5)
            stats = bridge._realtime.stats()
            self.assertEqual(stats["applied"], 0)
            self.assertEqual(stats["revokedBeforeApply"], 4)
            self.assertEqual(stats["applyFailures"], 4)
            self.assertEqual(stats["pending"], 0)
        finally:
            bridge.disconnect()

    def test_wire_numbers_are_written_as_javascript_writes_them(self):
        # The host signs and checks the same text: a parameter at 0.0000022 once broke every snapshot.
        from ableton_mcp_remote_script import _js_number
        for value, text in [(0.0000022411345526052173, "0.0000022411345526052173"), (5e-05, "0.00005"), (1e-06, "0.000001"), (1e-07, "1e-7"), (1.5e-07, "1.5e-7"), (0.1, "0.1"), (123.456, "123.456"), (1.5e21, "1.5e+21"), (-0.00001, "-0.00001"), (2.5e-300, "2.5e-300")]:
            self.assertEqual(_js_number(value), text)
        self.assertEqual(AuthenticatedRemoteScript._canonical({"value": 0.00005, "whole": 3.0}), '{"value":0.00005,"whole":3}')

    def test_racks_nested_three_deep_still_snapshot_and_sign(self):
        # A device in a rack in a rack's chain, and one more: once past the wire's depth, no snapshot could be sent.
        song = FakeSong(); leaf = FakeDevice(); leaf.parameters[0].value_items = ["Off", "On"]; device = leaf
        for level in range(3):
            chain = type("Chain", (), {"name": f"Chain {level}", "devices": [device], "mute": False, "solo": False})()
            rack = FakeDevice(); rack.name = f"Rack {level}"; rack.can_have_chains = True; rack.chains = [chain]; device = rack
        song.tracks[0].devices = [device]
        snapshot = LiveObjectMapper(song).snapshot()
        frame = {"version": 1, "id": "async-1", "ok": True, "result": snapshot}
        self.assertTrue(AuthenticatedRemoteScript._canonical(frame))

    def test_stepped_parameters_report_whole_steps_and_take_the_nearest_one(self):
        mapper = LiveObjectMapper(FakeSong())
        switch = mapper.song.tracks[0].devices[0].parameters[0]
        del switch.quantization; switch.is_quantized = True; switch.value = 0.0; switch.value_items = ["Off", "On"]
        row = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        self.assertEqual(row["quantization"], 1.0); self.assertEqual(row["valueItems"], ["Off", "On"])
        self.assertEqual(mapper._set_parameter_value(row["ref"], 0.75)["value"], 1.0, "between steps: the nearest one")
        self.assertEqual(mapper._set_parameter_value(row["ref"], 1.0)["value"], 1.0)
        switch.is_quantized = False; switch.value = 0.25
        self.assertEqual(mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]["quantization"], 0.0)

    def test_a_whole_number_parameter_takes_the_nearest_whole_number(self):
        # A scale's root note, a MIDI controller's 0-127: Live keeps whole numbers without marking them stepped.
        class WholeParameter(FakeParameter):
            @property
            def value(self): return self._value
            @value.setter
            def value(self, value): self._value = float(round(value))
        mapper = LiveObjectMapper(FakeSong()); knob = WholeParameter(); knob._value = 0.0; knob.min = 0.0; knob.max = 127.0; del knob.quantization; knob.is_quantized = False
        mapper.song.tracks[0].devices[0].parameters = [knob]; row = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        self.assertEqual(mapper._set_parameter_value(row["ref"], 95.25)["value"], 95.0); self.assertEqual(knob.value, 95.0)
        knob.min = 0.0; knob.max = 1.0; knob._value = 0.0
        class Stuck(FakeParameter):
            value = property(lambda self: 0.0, lambda self, value: None)
        stuck = Stuck(); del stuck.quantization; stuck.is_quantized = False; mapper.song.tracks[0].devices[0].parameters = [stuck]; row = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        with self.assertRaisesRegex(ValueError, "not confirmed"): mapper._set_parameter_value(row["ref"], 0.75)

    def test_real_mapper_authority_matches_filtered_bounded_snapshot_siblings(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM); tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        udp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM); udp_probe.bind(("127.0.0.1", 0)); realtime_port = udp_probe.getsockname()[1]; udp_probe.close()
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
        try:
            hidden = FakeParameter(); hidden.min = None
            bridge.mapper.song.tracks[0].devices[0].parameters = [hidden] + [FakeParameter() for _ in range(256)]
            with patch.object(remote_module, "MAX_DISCOVERY_COLLECTION_LENGTH", 256), self.assertRaisesRegex(ValueError, "complete-state bound"): bridge.mapper.snapshot()
            bridge.mapper.song.tracks[0].devices[0].parameters = [hidden] + [FakeParameter() for _ in range(255)]
            rows = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"]
            self.assertEqual(len(rows), 255)
            target_ref = rows[0]["ref"]; authority = bridge.mapper._realtime_parameter_authority(target_ref)
            self.assertEqual(authority["siblings"], [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in rows])
            armed = bridge._realtime_op("realtime.arm", {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [target_ref], "targetAuthorities": [authority], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}})
            self.assertTrue(bridge._realtime.stats()["armed"]); self.assertEqual(armed["parameterRefs"], [target_ref]); bridge._realtime.disarm()
            bridge.mapper.song.tracks[0].devices = [FakeDevice() for _ in range(5)]
            for device in bridge.mapper.song.tracks[0].devices: device.parameters = [FakeParameter() for _ in range(256)]
            device_rows = bridge.mapper.snapshot()["tracks"][0]["devices"]
            self.assertEqual(bridge.mapper._realtime_parameter_authority(device_rows[2]["parameters"][0]["ref"])["parameterIdentity"], device_rows[2]["parameters"][0]["objectIdentity"])
            # Only the device holding the parameter costs its parameters: a big Set's other devices don't refuse it.
            self.assertEqual(bridge.mapper._realtime_parameter_authority(device_rows[4]["parameters"][0]["ref"])["parameterIdentity"], device_rows[4]["parameters"][0]["objectIdentity"])
            target_track = bridge.mapper.song.tracks[0]; crowd = []
            for _ in range(65):
                track = FakeTrack(); track.devices = [FakeDevice() for _ in range(256)]; crowd.append(track)
            # A big Set is never refused for its size: 65 tracks of 256 devices around the target change nothing.
            bridge.mapper.song.tracks = [target_track] + crowd
            self.assertEqual(bridge.mapper._realtime_parameter_authority(device_rows[4]["parameters"][0]["ref"])["parameterIdentity"], device_rows[4]["parameters"][0]["objectIdentity"])
            # The reference is positional: with the target moved behind them, it names another track's device, not the parameter.
            bridge.mapper.song.tracks = crowd + [target_track]
            with self.assertRaisesRegex(ValueError, "no longer in the authoritative hierarchy"): bridge.mapper._realtime_parameter_authority(device_rows[4]["parameters"][0]["ref"])
            bridge.mapper.song.tracks = [target_track]
            rack = FakeDevice(); rack.can_have_chains = True; rack.chains = []; rack.macros = [rack.parameters[0]]; bridge.mapper.song.tracks[0].devices = [rack]
            rack_row = bridge.mapper.snapshot()["tracks"][0]["devices"][0]; macro_ref = rack_row["macros"][0]["ref"]; macro_authority = bridge.mapper._realtime_parameter_authority(macro_ref)
            self.assertEqual(macro_authority["ref"], macro_ref); self.assertEqual([row["ref"] for row in macro_authority["siblings"]], [macro_ref])
            macro_arm = bridge._realtime_op("realtime.arm", {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [macro_ref], "targetAuthorities": [macro_authority], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}})
            self.assertEqual(macro_arm["parameterRefs"], [macro_ref]); bridge._realtime.disarm()
            oversized_rack = FakeDevice(); oversized_rack.can_have_chains = True; oversized_rack.macros = []; oversized_rack.chains = [type("Chain", (), {"devices": []})() for _ in range(257)]
            bridge.mapper.song.tracks[0].devices = [oversized_rack, FakeDevice()]; later_ref = bridge.mapper.snapshot()["tracks"][0]["devices"][1]["parameters"][0]["ref"]
            with patch.object(remote_module, "MAX_DISCOVERY_COLLECTION_LENGTH", 256), self.assertRaises(ValueError): bridge.mapper._realtime_parameter_authority(later_ref)
        finally:
            bridge.disconnect()

    def test_real_mapper_track_reorder_before_arm_refuses_stale_host_authority(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM); tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        udp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM); udp_probe.bind(("127.0.0.1", 0)); realtime_port = udp_probe.getsockname()[1]; udp_probe.close()
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
        try:
            snapshot = bridge.mapper.snapshot(); parameter_ref = snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]; stale_authority = bridge.mapper._realtime_parameter_authority(parameter_ref)
            bridge.mapper.song.tracks.insert(0, FakeTrack())
            with self.assertRaises(ValueError): bridge._realtime_op("realtime.arm", {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [parameter_ref], "targetAuthorities": [stale_authority], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}})
            self.assertFalse(bridge._realtime.stats()["armed"])
        finally:
            bridge.disconnect()

    def test_real_mapper_track_reorder_revokes_parameter_authority(self):
        import socket as _socket
        tcp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM); tcp_probe.bind(("127.0.0.1", 0)); tcp_port = tcp_probe.getsockname()[1]; tcp_probe.close()
        udp_probe = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM); udp_probe.bind(("127.0.0.1", 0)); realtime_port = udp_probe.getsockname()[1]; udp_probe.close()
        bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": tcp_port, "realtimePort": realtime_port, "secret": "x" * 40})
        try:
            snapshot = bridge.mapper.snapshot(); parameter_ref = snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]; parameter = bridge.mapper._resolve_parameter(parameter_ref); prior = parameter.value; target_authority = bridge.mapper._realtime_parameter_authority(parameter_ref)
            armed = bridge._realtime_op("realtime.arm", {"ttlMs": 30000, "channels": ["udp-json"], "parameterRefs": [parameter_ref], "targetAuthorities": [target_authority], "outputSafety": {"safe": True, "provenance": "unit-test-operator"}})
            bridge._realtime._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref=parameter_ref, value=0.25))
            bridge.mapper.song.tracks.insert(0, FakeTrack()); bridge.queue.drain()
            stats = bridge._realtime.stats(); self.assertFalse(stats["armed"]); self.assertEqual(stats["applyFailures"], 1); self.assertEqual(parameter.value, prior)
        finally:
            bridge.disconnect()

    def test_arm_bounds_endpoint_channel_and_drop_accounting(self):
        plane = self._plane()
        try:
            with self.assertRaises(ValueError):
                self._arm(plane, 500, ["udp-json"], ["p"])
            with self.assertRaises(ValueError):
                self._arm(plane, 30000, ["udp-json", "udp-json"], ["p"])
            stale_authority = plane._bridge.mapper._realtime_parameter_authority("p"); plane._bridge.mapper.authority_generation += 1
            with self.assertRaises(ValueError): plane.arm(30000, ["udp-json"], ["p"], None, [stale_authority])
            plane._bridge.mapper.authority_generation -= 1
            armed = self._arm(plane, 30000, ["udp-json", "xy"], ["p", "x", "y"], [41000])
            self.assertEqual(armed["host"], "127.0.0.1")
            self.assertEqual(armed["packetLimitBytes"], 512)
            plane._handle(b"not-json", ("127.0.0.1", 41000))
            plane._handle(self._json(token="w" * 32, seq=1, channel="udp-json", op="parameter.set", ref="p", value=1), ("127.0.0.1", 41000))
            plane._handle(self._json(token=armed["token"], seq=1, channel="max", op="parameter.set", ref="p", value=1), ("127.0.0.1", 41000))
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=1), ("127.0.0.1", 42000))
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="not-allowed", value=1), ("127.0.0.1", 41000))
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=1, sentAtMs=time.time() * 1000), ("127.0.0.1", 41000))
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=1), ("127.0.0.1", 41000))
            plane._handle(self._json(token=armed["token"], seq=4, channel="xy", op="xy.set", xRef="x", x=0.2, yRef="y", y=0.8, sentAtMs=time.time() * 1000), ("127.0.0.1", 41000))
            stats = plane.stats()
            self.assertEqual(stats["accepted"], 2)
            self.assertEqual(stats["applied"], 2)
            self.assertEqual(stats["droppedUnarmed"], 2)
            self.assertEqual(stats["droppedEndpoint"], 1)
            self.assertEqual(stats["droppedTarget"], 1)
            self.assertEqual(stats["droppedInvalid"], 1)
            self.assertEqual(stats["droppedReplay"], 1)
            self.assertEqual(stats["sequenceGaps"], 2)
            self.assertEqual(stats["lastSequence"], 4)
            self.assertGreaterEqual(stats["jitterMs"], 0)
            plane.disarm()
            plane._handle(self._json(token=armed["token"], seq=6, channel="udp-json", op="emergency-stop"), ("127.0.0.1", 41000))
            self.assertEqual(plane.stats()["droppedUnarmed"], 3)
        finally:
            plane.close()

    def test_osc_max_xy_queue_and_parameter_bounds(self):
        plane = self._plane()
        try:
            armed = self._arm(plane, 30000, ["osc", "max", "xy"], ["p", "x", "y"])
            plane._handle(self._osc_parameter(armed["token"], 1, "p", 0.25))
            plane._handle(self._json(token=armed["token"], seq=2, channel="max", op="parameter.set", ref="p", value=2.0))
            plane._handle(self._json(token=armed["token"], seq=3, channel="xy", op="xy.set", xRef="x", x=0.3, yRef="y", y=0.7))
            plane._bridge.queue.accept = False
            plane._handle(self._json(token=armed["token"], seq=4, channel="max", op="emergency-stop"))
            stats = plane.stats()
            self.assertEqual(stats["accepted"], 3)
            self.assertEqual(stats["applied"], 2)
            self.assertEqual(stats["applyFailures"], 1)
            self.assertEqual(stats["droppedQueueFull"], 1)
            self.assertAlmostEqual(plane._bridge.mapper.parameters["p"].value, 0.25)
            self.assertAlmostEqual(plane._bridge.mapper.parameters["x"].value, 0.3)
            self.assertAlmostEqual(plane._bridge.mapper.parameters["y"].value, 0.7)
            plane._handle(b"x" * 513)
            self.assertEqual(plane.stats()["droppedInvalid"], 1)
        finally:
            plane.close()

    def test_parameter_topology_change_revokes_armed_generation_before_apply(self):
        plane = self._plane()
        try:
            queue = plane._bridge.queue; queue.defer = True
            armed = self._arm(plane, 30000, ["udp-json"], ["p"])
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=0.75))
            plane._bridge.mapper.authority_generation += 1
            with self.assertRaises(ValueError): queue.calls.pop(0)()
            stats = plane.stats(); self.assertFalse(stats["armed"]); self.assertEqual(stats["applied"], 0); self.assertEqual(stats["applyFailures"], 1); self.assertEqual(stats["revokedBeforeApply"], 1)
            self.assertEqual(plane._bridge.mapper.parameters["p"].value, 0.0)
        finally:
            plane.close()

    def test_disarm_expiry_and_rearm_fence_accepted_callbacks(self):
        plane = self._plane()
        try:
            queue = plane._bridge.queue
            queue.defer = True
            armed = self._arm(plane, 30000, ["udp-json"], ["p"])
            plane._handle(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=0.75))
            self.assertEqual(plane.stats()["accepted"], 1)
            self.assertEqual(plane.stats()["pending"], 1)
            plane.disarm()
            with self.assertRaises(ValueError):
                queue.calls.pop(0)()
            self.assertEqual(plane._bridge.mapper.parameters["p"].value, 0.0)

            expired = self._arm(plane, 30000, ["udp-json"], ["p"])
            plane._handle(self._json(token=expired["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=0.5))
            with plane._lock:
                token, _, channels, ports, parameters = plane._armed
                plane._armed = (token, time.time() - 1, channels, ports, parameters)
            with self.assertRaises(ValueError):
                queue.calls.pop(0)()

            old = self._arm(plane, 30000, ["udp-json"], ["p"])
            plane._handle(self._json(token=old["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=0.4))
            self._arm(plane, 30000, ["udp-json"], ["p"])
            with self.assertRaises(ValueError):
                queue.calls.pop(0)()
            stats = plane.stats()
            self.assertEqual(stats["revokedBeforeApply"], 3)
            self.assertEqual(stats["applyFailures"], 3)
            self.assertEqual(stats["applied"], 0)
            self.assertEqual(stats["pending"], 0)
        finally:
            plane.close()

    def test_actual_udp_bounds_and_receiver_survives_queue_failure(self):
        import socket as _socket
        from ableton_mcp_remote_script import validate_operation_payload
        plane = self._plane()
        sender = _socket.socket(_socket.AF_INET, _socket.SOCK_DGRAM)
        sender.bind(("127.0.0.1", 0))
        try:
            armed = self._arm(plane, 30000, ["udp-json"], ["p"], [sender.getsockname()[1]])
            sender.sendto(b"x" * 513, ("127.0.0.1", plane.port))
            plane._bridge.queue.raise_once = True
            sender.sendto(self._json(token=armed["token"], seq=1, channel="udp-json", op="parameter.set", ref="p", value=0.2), ("127.0.0.1", plane.port))
            sender.sendto(self._json(token=armed["token"], seq=2, channel="udp-json", op="parameter.set", ref="p", value=0.3), ("127.0.0.1", plane.port))
            deadline = time.time() + 3
            while time.time() < deadline and plane.stats()["accepted"] < 1:
                time.sleep(0.02)
            stats = plane.stats()
            self.assertGreaterEqual(stats["droppedInvalid"], 1)
            self.assertGreaterEqual(stats["droppedBeforeDispatch"], 1)
            self.assertEqual(stats["accepted"], 1)
            self.assertEqual(stats["applied"], 1)
            self.assertTrue(plane._thread.is_alive())
            validate_operation_payload("realtime.stats", "result", stats)
        finally:
            sender.close()
            plane.close()

    def test_rate_limit_drops_bursts_without_replay_gap_double_counting(self):
        plane = self._plane()
        try:
            armed = self._arm(plane, 30000, ["udp-json"], ["p"])
            for seq in range(1, 41):
                plane._handle(self._json(token=armed["token"], seq=seq, channel="udp-json", op="parameter.set", ref="p", value=0.5))
            stats = plane.stats()
            self.assertGreater(stats["droppedRateLimited"], 0)
            self.assertLess(stats["accepted"], 40)
            self.assertEqual(stats["sequenceGaps"], 0)
            self.assertEqual(stats["lastSequence"], 40)
        finally:
            plane.close()


class ViewLocatorClipExpansionTests(unittest.TestCase):
    def test_clip_set_mutes_colors_and_loops_midi_clips_with_fail_closed_rollback(self):
        song = FakeSong(); clip = FakeClip(8.0)
        clip.is_audio_clip = False; clip.muted = False; clip.color_index = 1; clip.looping = False; clip.loop_start = 0.0; clip.loop_end = 8.0
        song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertEqual((row["muted"], row["colorIndex"], row["looping"], row["loopStart"], row["loopEnd"]), (False, 1, False, 0.0, 8.0))
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        def payload(**changes):
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], **changes, "expectedObjectIdentity": row["objectIdentity"],
                    "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        stale = payload(muted=True, colorIndex=5, looping=True, loopStart=1.0, loopEnd=5.0)
        result = mapper.invoke("clip.set", stale)
        self.assertTrue(result["changed"]); validate_operation_payload("clip.set", "result", result)
        self.assertTrue(clip.muted); self.assertEqual(clip.color_index, 5); self.assertTrue(clip.looping); self.assertEqual((clip.loop_start, clip.loop_end), (1.0, 5.0))
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("clip.set", stale)
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("clip.set", payload())
        with self.assertRaisesRegex(ValueError, "colorIndex is invalid"): mapper.invoke("clip.set", payload(colorIndex=70))
        with self.assertRaisesRegex(ValueError, "loopStart must not exceed loopEnd"): mapper.invoke("clip.set", payload(loopStart=7.0, loopEnd=6.0))

    def test_clip_set_rejects_loop_edits_on_audio_clips_and_rolls_back_exactly(self):
        song = FakeSong(); clip = FakeClip(4.0)
        clip.is_audio_clip = True; clip.muted = False; clip.color_index = 2; clip.looping = True; clip.loop_start = 0.0; clip.loop_end = 4.0
        song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertEqual((row["loopStart"], row["loopEnd"]), (0.0, 4.0))
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        def payload(**changes):
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], **changes, "expectedObjectIdentity": row["objectIdentity"],
                    "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        with self.assertRaisesRegex(ValueError, "audio clip loop editing"): mapper.invoke("clip.set", payload(looping=False))
        result = mapper.invoke("clip.set", payload(muted=True))
        self.assertTrue(result["changed"]); self.assertTrue(clip.muted)

        class FailingClip(FakeClip):
            @property
            def muted(self): return self._muted
            @muted.setter
            def muted(self, value):
                if value is True: raise RuntimeError("Live rejected the write")
                self._muted = value
        failing = FailingClip(4.0); failing._muted = False; failing.is_audio_clip = False; failing.color_index = 3; failing.looping = False; failing.loop_start = 0.0; failing.loop_end = 4.0
        song.tracks[0].clip_slots[0].clip = failing; mapper = LiveObjectMapper(song); failing_row = mapper.snapshot()["tracks"][0]["clips"][0]
        def failing_payload(**changes):
            current = mapper.get(failing_row["ref"])
            return {"ref": failing_row["ref"], **changes, "expectedObjectIdentity": failing_row["objectIdentity"],
                    "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(failing_row["ref"])).encode()).hexdigest(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        with self.assertRaises(RuntimeError):
            mapper.invoke("clip.set", failing_payload(colorIndex=9, muted=True))
        self.assertEqual(failing.color_index, 3); self.assertFalse(failing.muted)

    def test_clip_set_arrangement_clip_uses_arrangement_authority(self):
        song = FakeSong(); track = song.tracks[0]
        clip = FakeClip(4.0); clip.name = "Arr"; clip.start_time = 4.0; clip.is_audio_clip = False; clip.muted = False; clip.color_index = 1; clip.looping = True; clip.loop_start = 0.0; clip.loop_end = 4.0
        track.arrangement_clips = [clip]
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["arrangement"]["clips"][0]
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        current = mapper.get(row["ref"])
        args = {"ref": row["ref"], "muted": True, "expectedObjectIdentity": row["objectIdentity"],
                "expectedAuthorityRevision": mapper._arrangement_clip_authority_revision(row["ref"]),
                "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        result = mapper.invoke("clip.set", args)
        self.assertTrue(result["changed"]); self.assertTrue(clip.muted)

    def test_clip_set_launch_legato_ram_and_velocity_fields_with_gating(self):
        song = FakeSong(); clip = FakeClip(8.0)
        clip.is_audio_clip = False; clip.muted = False; clip.color_index = 1; clip.looping = False; clip.loop_start = 0.0; clip.loop_end = 8.0
        clip.launch_mode = 0; clip.launch_quantization = 4; clip.legato = False; clip.velocity_amount = 0.0; clip.is_playing = False; clip.is_triggered = False
        song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertEqual((row["launchMode"], row["launchQuantization"], row["legato"], row["velocityAmount"], row["ramMode"]), (0, 4, False, 0.0, None))
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        def payload(**changes):
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], **changes, "expectedObjectIdentity": row["objectIdentity"],
                    "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        result = mapper.invoke("clip.set", payload(launchMode=1, launchQuantization=14, legato=True, velocityAmount=0.5))
        self.assertTrue(result["changed"]); validate_operation_payload("clip.set", "result", result)
        self.assertEqual((clip.launch_mode, clip.launch_quantization, clip.legato, clip.velocity_amount), (1, 14, True, 0.5))
        with self.assertRaisesRegex(ValueError, "launchMode is invalid"): mapper.invoke("clip.set", payload(launchMode=4))
        with self.assertRaisesRegex(ValueError, "launchQuantization is invalid"): mapper.invoke("clip.set", payload(launchQuantization=15))
        with self.assertRaisesRegex(ValueError, "velocityAmount is invalid"): mapper.invoke("clip.set", payload(velocityAmount=1.5))
        with self.assertRaisesRegex(ValueError, "legato is invalid"): mapper.invoke("clip.set", payload(legato=1))
        with self.assertRaisesRegex(ValueError, "only available on audio clips"): mapper.invoke("clip.set", payload(ramMode=True))
        clip.is_playing = True
        with self.assertRaisesRegex(ValueError, "playing or triggered"): mapper.invoke("clip.set", payload(launchMode=2))
        clip.is_playing = False

    def test_clip_set_audio_ram_mode_and_velocity_gating(self):
        song = FakeSong(); clip = FakeClip(4.0)
        clip.is_audio_clip = True; clip.muted = False; clip.color_index = 2; clip.looping = True; clip.loop_start = 0.0; clip.loop_end = 4.0
        clip.launch_mode = 0; clip.launch_quantization = 4; clip.ram_mode = False; clip.is_playing = False; clip.is_triggered = False
        song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        def payload(**changes):
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], **changes, "expectedObjectIdentity": row["objectIdentity"],
                    "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        result = mapper.invoke("clip.set", payload(ramMode=True))
        self.assertTrue(result["changed"]); self.assertTrue(clip.ram_mode)
        with self.assertRaisesRegex(ValueError, "only available on MIDI clips"): mapper.invoke("clip.set", payload(velocityAmount=0.5))
        del clip.ram_mode
        with self.assertRaisesRegex(ValueError, "unavailable on this clip"): mapper.invoke("clip.set", payload(ramMode=False))

    def test_clip_set_multi_field_failure_rolls_back_launch_fields_exactly(self):
        song = FakeSong()
        class LegatoRefusingClip(FakeClip):
            @property
            def legato(self): return self._legato
            @legato.setter
            def legato(self, value):
                if value is True: raise RuntimeError("Live rejected the write")
                self._legato = value
        clip = LegatoRefusingClip(8.0); clip._legato = False
        clip.is_audio_clip = False; clip.muted = False; clip.color_index = 1; clip.looping = False; clip.loop_start = 0.0; clip.loop_end = 8.0
        clip.launch_mode = 0; clip.launch_quantization = 4; clip.velocity_amount = 0.0; clip.is_playing = False; clip.is_triggered = False
        song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        current = mapper.get(row["ref"])
        args = {"ref": row["ref"], "launchMode": 3, "legato": True, "expectedObjectIdentity": row["objectIdentity"],
                "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical(mapper._session_clip_authority(row["ref"])).encode()).hexdigest(),
                "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        with self.assertRaises(RuntimeError):
            mapper.invoke("clip.set", args)
        self.assertEqual((clip.launch_mode, clip.launch_quantization, clip.legato), (0, 4, False))

    def test_locator_jump_navigates_cue_points(self):
        song = FakeArrangementSong()
        song.cue_points = [FakeLocator(8.0, "A"), FakeLocator(16.0, "B")]
        def jump_next():
            later = [locator.time for locator in song.cue_points if locator.time > song.current_song_time]
            if later: song.current_song_time = min(later)
        def jump_previous():
            earlier = [locator.time for locator in song.cue_points if locator.time < song.current_song_time - 1e-9]
            song.current_song_time = max(earlier) if earlier else 0.0
        song.jump_to_next_cue = jump_next; song.jump_to_prev_cue = jump_previous
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("locator.jump"))
        result = mapper.invoke("locator.jump", {"direction": "next"})
        self.assertEqual((result["before"], result["position"]), (0.0, 8.0)); validate_operation_payload("locator.jump", "result", result)
        self.assertEqual(mapper.invoke("locator.jump", {"direction": "previous"})["position"], 0.0)
        with self.assertRaisesRegex(ValueError, "direction is invalid"): mapper.invoke("locator.jump", {"direction": "sideways"})
        without = LiveObjectMapper(FakeSong())
        self.assertFalse(without._operation_supported("locator.jump"))
        with self.assertRaisesRegex(ValueError, "unavailable"): without.invoke("locator.jump", {"direction": "next"})

    def test_view_set_and_control_use_application_view_with_readback(self):
        class FakeAppView:
            def __init__(self): self.visible = "Session"; self.zooms = []; self.scrolls = []
            def show_view(self, name): self.visible = name
            def is_view_visible(self, name): return self.visible == name
            def zoom_view(self, direction, surface, animate): self.zooms.append((direction, surface, animate))
            def scroll_view(self, direction, surface, animate): self.scrolls.append((direction, surface, animate))
        class FakeApplication: pass
        song = FakeSong(); song.view = type("SongView", (), {"follow_song": False})()
        song.tracks[0].view = type("TrackView", (), {"is_collapsed": False})()
        application = FakeApplication(); application.view = FakeAppView()
        mapper = LiveObjectMapper(song); mapper._application = lambda: application
        self.assertTrue(mapper._operation_supported("view.set")); self.assertTrue(mapper._operation_supported("view.control"))
        result = mapper.invoke("view.set", {"view": "Arranger"})
        self.assertEqual(result, {"view": "Arranger", "visible": True}); validate_operation_payload("view.set", "result", result)
        application.view.is_view_visible = lambda name: False
        with self.assertRaisesRegex(ValueError, "not confirmed"): mapper.invoke("view.set", {"view": "Session"})
        self.assertEqual(mapper.invoke("view.control", {"action": "zoom-in"}), {"action": "zoom-in", "done": True})
        self.assertEqual(application.view.zooms, [(1, "Arranger", False)])
        self.assertEqual(mapper.invoke("view.control", {"action": "scroll-right"}), {"action": "scroll-right", "done": True})
        self.assertEqual(application.view.scrolls, [(1, "Arranger", False)])
        mapper.invoke("view.control", {"action": "follow-on"}); self.assertTrue(song.view.follow_song)
        mapper.invoke("view.control", {"action": "follow-off"}); self.assertFalse(song.view.follow_song)
        track_ref = mapper.snapshot()["tracks"][0]["ref"]
        mapper.invoke("view.control", {"action": "collapse-track", "trackRef": track_ref}); self.assertTrue(song.tracks[0].view.is_collapsed)
        mapper.invoke("view.control", {"action": "expand-track", "trackRef": track_ref}); self.assertFalse(song.tracks[0].view.is_collapsed)
        with self.assertRaisesRegex(ValueError, "action is invalid"): mapper.invoke("view.control", {"action": "detonate"})
        with self.assertRaisesRegex(ValueError, "track reference is stale"): mapper.invoke("view.control", {"action": "collapse-track", "trackRef": "bogus"})
        without = LiveObjectMapper(FakeSong())
        self.assertFalse(without._operation_supported("view.set")); self.assertFalse(without._operation_supported("view.control"))
        with self.assertRaisesRegex(ValueError, "unavailable"): without.invoke("view.set", {"view": "Arranger"})

    def test_arrangement_audio_clip_create_places_file_and_cleans_up_exactly(self):
        song = FakeSong(); track = song.tracks[0]; track.arrangement_clips = []
        def create_audio_clip(file_path, position):
            clip = FakeClip(4.0); clip.start_time = position; clip.file_path = file_path; track.arrangement_clips.append(clip); return clip
        track.create_audio_clip = create_audio_clip
        track.delete_clip = lambda candidate: track.arrangement_clips.remove(candidate)
        mapper = LiveObjectMapper(song); track_row = mapper.snapshot()["tracks"][0]
        args = {"trackRef": track_row["ref"], "filePath": "/tmp/demo.wav", "position": 8.0, "name": "Imported",
                "expectedTrackIdentity": track_row["objectIdentity"], "expectedCollectionRevision": mapper._arrangement_collection_revision(track, 0)}
        result = mapper.invoke("arrangement.audio-clip.create", args)
        self.assertEqual((result["filePath"], result["start"], result["length"], result["name"]), ("/tmp/demo.wav", 8.0, 4.0, "Imported"))
        validate_operation_payload("arrangement.audio-clip.create", "result", result)
        self.assertEqual(len(track.arrangement_clips), 1)
        with self.assertRaisesRegex(ValueError, "collection changed since preview"):
            mapper.invoke("arrangement.audio-clip.create", args)
        self.assertEqual(len(track.arrangement_clips), 1)

        def broken_creator(file_path, position):
            clip = FakeClip(4.0); clip.start_time = position; clip.file_path = ""; track.arrangement_clips.append(clip); return clip
        track.create_audio_clip = broken_creator
        broken_args = dict(args, expectedCollectionRevision=mapper._arrangement_collection_revision(track, 0))
        with self.assertRaisesRegex(ValueError, "file path was not confirmed"):
            mapper.invoke("arrangement.audio-clip.create", broken_args)
        self.assertEqual(len(track.arrangement_clips), 1)
        with self.assertRaisesRegex(ValueError, "filePath is invalid"):
            mapper.invoke("arrangement.audio-clip.create", dict(broken_args, filePath=""))


class FakeWarpMarker:
    def __init__(self, beat, sample): self.beat_time = beat; self.sample_time = sample


class FakeAudioClipFull(FakeClip):
    def __init__(self, length):
        super().__init__(length)
        self.is_audio_clip = True
        self.warp_markers = [FakeWarpMarker(1.0, 44100.0), FakeWarpMarker(3.0, 132300.0)]
        self.file_path = "/tmp/a.wav"
        self.looping = True; self.loop_start = 0.0; self.loop_end = length
        self.muted = False; self.color_index = 0
        self.available_warp_modes = [0, 1, 2, 3, 4, 6]
        self.sample_length = 176400.0

    def add_warp_marker(self, spec):
        if not isinstance(spec, dict) or "beat_time" not in spec: raise TypeError("add_warp_marker takes a dict")
        beat = float(spec["beat_time"]); sample = float(spec.get("sample_time", beat * 44100.0))
        marker = FakeWarpMarker(beat, sample); self.warp_markers.append(marker); self.warp_markers.sort(key=lambda item: item.beat_time); return marker

    def move_warp_marker(self, beat, distance):
        for marker in self.warp_markers:
            if marker.beat_time == beat:
                marker.beat_time = beat + distance; marker.sample_time = (beat + distance) * 44100.0; self.warp_markers.sort(key=lambda item: item.beat_time); return
        raise RuntimeError("no marker at beat")

    def remove_warp_marker(self, beat):
        if isinstance(beat, FakeWarpMarker): raise TypeError("remove_warp_marker takes a beat-time float")
        for marker in list(self.warp_markers):
            if marker.beat_time == beat: self.warp_markers.remove(marker); return
        raise RuntimeError("no marker at beat")


class AudioWarpNoteExpansionTests(unittest.TestCase):
    def _mapper_with_audio_clip(self):
        song = FakeSong(); clip = FakeAudioClipFull(4.0); song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        return song, clip, mapper, row

    def _fences(self, mapper, row, clip):
        return {"ref": row["ref"], "expectedClipAuthorityDigest": mapper._clip_authority_digest(row["ref"]),
                "expectedMarkerCollectionRevision": mapper._warp_marker_collection_revision(clip)}

    def test_warp_marker_rows_and_audio_metadata_are_exposed(self):
        _, clip, mapper, row = self._mapper_with_audio_clip()
        self.assertEqual(row["warpMarkers"], [{"beatTime": 1.0, "sampleTime": 44100.0}, {"beatTime": 3.0, "sampleTime": 132300.0}])
        self.assertEqual(row["availableWarpModes"], [0, 1, 2, 3, 4, 6]); self.assertEqual(row["sampleLength"], 176400.0)
        result = mapper.invoke("audio.warp-marker.read", {"ref": row["ref"]})
        self.assertEqual([marker["beatTime"] for marker in result["markers"]], [1.0, 3.0]); validate_operation_payload("audio.warp-marker.read", "result", result)
        self.assertTrue(mapper._operation_supported("audio.warp-marker.read")); self.assertTrue(mapper._operation_supported("audio.warp-marker.add"))
        self.assertIn("warp", mapper.capabilities())

    def test_warp_marker_add_move_delete_with_fences_and_refusals(self):
        _, clip, mapper, row = self._mapper_with_audio_clip()
        result = mapper.invoke("audio.warp-marker.add", {**self._fences(mapper, row, clip), "beatTime": 2.0})
        self.assertTrue(result["changed"]); validate_operation_payload("audio.warp-marker.add", "result", result)
        self.assertEqual([marker.beat_time for marker in clip.warp_markers], [1.0, 2.0, 3.0])
        with self.assertRaisesRegex(ValueError, "already exists"): mapper.invoke("audio.warp-marker.add", {**self._fences(mapper, row, clip), "beatTime": 2.0})
        stale = self._fences(mapper, row, clip); stale["expectedMarkerCollectionRevision"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "collection changed"): mapper.invoke("audio.warp-marker.add", {**stale, "beatTime": 4.0})
        moved = mapper.invoke("audio.warp-marker.move", {**self._fences(mapper, row, clip), "beatTime": 1.0, "distance": 0.5})
        self.assertTrue(moved["changed"]); self.assertEqual([marker.beat_time for marker in clip.warp_markers], [1.5, 2.0, 3.0])
        with self.assertRaisesRegex(ValueError, "no warp marker"): mapper.invoke("audio.warp-marker.move", {**self._fences(mapper, row, clip), "beatTime": 1.0, "distance": 0.5})
        with self.assertRaisesRegex(ValueError, "collides"): mapper.invoke("audio.warp-marker.move", {**self._fences(mapper, row, clip), "beatTime": 1.5, "distance": 0.5})
        deleted = mapper.invoke("audio.warp-marker.delete", {**self._fences(mapper, row, clip), "beatTime": 3.0})
        self.assertTrue(deleted["changed"]); self.assertEqual([marker.beat_time for marker in clip.warp_markers], [1.5, 2.0])

    def test_warp_marker_acknowledgement_loss_compensates_exactly(self):
        _, clip, mapper, row = self._mapper_with_audio_clip()
        original_move = clip.move_warp_marker
        calls = []
        def flaky_move(beat, distance):
            original_move(beat, distance)
            if not calls:
                calls.append(1); raise RuntimeError("ack lost")
        clip.move_warp_marker = flaky_move
        with self.assertRaisesRegex(RuntimeError, "ack lost"):
            mapper.invoke("audio.warp-marker.move", {**self._fences(mapper, row, clip), "beatTime": 1.0, "distance": 0.25})
        self.assertEqual([marker.beat_time for marker in clip.warp_markers], [1.0, 3.0])

    def test_session_audio_clip_create_with_file_authority_refusals(self):
        song = FakeSong(); track = song.tracks[0]; track.has_midi_input = False; slot = track.clip_slots[0]
        def create_audio_clip(path):
            clip = FakeClip(4.0); clip.is_audio_clip = True; clip.file_path = path; slot.clip = clip; return clip
        slot.create_audio_clip = create_audio_clip
        mapper = LiveObjectMapper(song); track_row = mapper.snapshot()["tracks"][0]; slot_row = track_row["clipSlots"][0]; scene_row = mapper.snapshot()["scenes"][0]
        args = {"trackRef": track_row["ref"], "sceneIndex": 0, "filePath": "/tmp/demo.wav", "name": "Imported",
                "expectedTrackIdentity": track_row["objectIdentity"], "expectedSlotRef": slot_row["ref"], "expectedSlotIdentity": slot_row["objectIdentity"],
                "expectedSceneRef": scene_row["ref"], "expectedSceneIdentity": scene_row["objectIdentity"]}
        self.assertTrue(mapper._operation_supported("session.audio-clip.create"))
        result = mapper.invoke("session.audio-clip.create", args)
        self.assertEqual((result["filePath"], result["name"], result["length"]), ("/tmp/demo.wav", "Imported", 4.0)); validate_operation_payload("session.audio-clip.create", "result", result)
        with self.assertRaisesRegex(ValueError, "occupied"): mapper.invoke("session.audio-clip.create", args)
        song2 = FakeSong(); slot2 = song2.tracks[0].clip_slots[0]; slot2.create_audio_clip = create_audio_clip
        mapper2 = LiveObjectMapper(song2); track2 = mapper2.snapshot()["tracks"][0]; slot2_row = track2["clipSlots"][0]; scene2 = mapper2.snapshot()["scenes"][0]
        relative = dict(args, trackRef=track2["ref"], filePath="demo.wav", expectedTrackIdentity=track2["objectIdentity"], expectedSlotRef=slot2_row["ref"], expectedSlotIdentity=slot2_row["objectIdentity"], expectedSceneRef=scene2["ref"], expectedSceneIdentity=scene2["objectIdentity"])
        with self.assertRaisesRegex(ValueError, "absolute path"): mapper2.invoke("session.audio-clip.create", relative)
        with self.assertRaisesRegex(ValueError, "not an audio track"): mapper2.invoke("session.audio-clip.create", dict(relative, filePath="/tmp/demo.wav"))

    def _clip_action_fences(self, mapper, row):
        current = mapper.get(row["ref"])
        state = hashlib.sha256(mapper._bounded_canonical({"isPlaying": current.get("isPlaying"), "length": current.get("length"), "loopStart": current.get("loopStart"), "loopEnd": current.get("loopEnd")}).encode()).hexdigest()
        return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._clip_authority_digest(row["ref"]), "expectedStateRevision": state}

    def test_clip_actions_crop_duplicate_and_scrub(self):
        song = FakeSong(); clip = FakeClip(4.0); clip.loop_start = 1.0; clip.loop_end = 3.0
        clip.crop = lambda: setattr(clip, "length", clip.loop_end - clip.loop_start)
        clip.duplicate_loop = lambda: setattr(clip, "length", clip.length * 2)
        clip.duplicate_region = lambda start, end, dest: setattr(clip, "length", clip.length + (end - start))
        clip.playing_position = 0.5
        clip.start_scrub = lambda position: setattr(clip, "playing_position", position)
        clip.stop_scrub = lambda: setattr(clip, "playing_position", 0.0)
        clip.move_playing_pos = lambda offset: setattr(clip, "playing_position", clip.playing_position + offset)
        song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertTrue(mapper._operation_supported("clip.action"))
        result = mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "crop", "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])})
        self.assertTrue(result["changed"]); self.assertEqual(clip.length, 2.0)
        result = mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "duplicate-loop", "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])})
        self.assertTrue(result["changed"]); self.assertEqual(clip.length, 4.0)
        result = mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "duplicate-region", "regionStart": 0.0, "regionEnd": 1.0, "destination": 4.0, "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])})
        self.assertTrue(result["changed"]); self.assertEqual(clip.length, 5.0)
        mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "scrub-start", "offset": 2.5})
        self.assertEqual(clip.playing_position, 2.5)
        mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "move-playing-position", "offset": 1.0})
        self.assertEqual(clip.playing_position, 3.5)
        mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "scrub-stop"})
        self.assertEqual(clip.playing_position, 0.0)
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "detonate"})
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("clip.action", {**self._clip_action_fences(mapper, row), "action": "crop", "expectedContentFingerprint": "0" * 64})

    def test_clip_actions_go_through_while_the_clip_plays(self):
        # A playing clip's playing_position moves every tick, so a crop previewed on one tick and applied
        # a few ticks later is the same crop. A loop changed in between is a real change and still refuses.
        song = FakeSong(); song.is_playing = True; song.current_song_time = 96.0
        clip = FakeClip(4.0); clip.loop_start = 1.0; clip.loop_end = 3.0; clip.is_playing = True; clip.playing_position = 1.25
        clip.crop = lambda: setattr(clip, "length", clip.loop_end - clip.loop_start)
        song.tracks[0].clip_slots[0].clip = clip
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song); mapper = bridge.mapper
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); bridge._executed_mutations = {}; bridge._executed_lock = threading.Lock(); holder = {}
        row = mapper.snapshot()["tracks"][0]["clips"][0]
        def play_on():
            song.current_song_time += 0.37; clip.playing_position = 1.0 + (clip.playing_position - 1.0 + 0.37) % 2.0
        def apply(args, key):
            request = {"operation": "clip.action", "transactionId": f"transaction-{key}", "args": args}
            preflight = bridge._dispatch_with_holder("preflight", request, holder); play_on()
            prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": key}, holder); play_on()
            return bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder)
        crop = {**self._clip_action_fences(mapper, row), "action": "crop", "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])}; play_on()
        self.assertTrue(apply(crop, "crop-while-playing")["changed"]); self.assertEqual(clip.length, 2.0)
        scrub = {**self._clip_action_fences(mapper, row), "action": "move-playing-position", "offset": 1.0}; play_on()
        clip.move_playing_pos = lambda offset: setattr(clip, "playing_position", clip.playing_position + offset)
        self.assertTrue(apply(scrub, "jump-while-playing")["changed"])
        stale = {**self._clip_action_fences(mapper, row), "action": "crop", "expectedContentFingerprint": mapper._mapped_fingerprint(row["ref"])}; clip.loop_end = 2.5
        with self.assertRaisesRegex(ValueError, "clip content changed since preview"): apply(stale, "crop-after-loop-change")
        self.assertEqual(clip.length, 2.0)
        stopped = {**self._clip_action_fences(mapper, row), "action": "move-playing-position", "offset": 1.0}; clip.is_playing = False
        with self.assertRaisesRegex(ValueError, "clip state changed since preview"): apply(stopped, "jump-after-clip-stopped")

    def test_automation_envelope_clear_counts_and_fences(self):
        song = FakeSong(); clip = FakeClip(4.0)
        envelope = type("Envelope", (), {})()
        clip._envelopes = {}
        clip.automation_envelope = lambda parameter: clip._envelopes.get(id(parameter))
        clip.clear_all_envelopes = lambda: clip._envelopes.clear()
        song.tracks[0].clip_slots[0].clip = clip; parameter = song.tracks[0].devices[0].parameters[0]
        clip._envelopes[id(parameter)] = envelope
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        revision = hashlib.sha256(mapper._bounded_canonical([True]).encode()).hexdigest()
        result = mapper.invoke("automation.envelope.clear", {"clipRef": row["ref"], "expectedAuthorityDigest": mapper._clip_authority_digest(row["ref"]), "expectedEnvelopesRevision": revision})
        self.assertEqual(result["cleared"], 1); validate_operation_payload("automation.envelope.clear", "result", result)
        self.assertEqual(clip._envelopes, {})
        with self.assertRaisesRegex(ValueError, "collection changed since preview"):
            mapper.invoke("automation.envelope.clear", {"clipRef": row["ref"], "expectedAuthorityDigest": mapper._clip_authority_digest(row["ref"]), "expectedEnvelopesRevision": revision})

    def test_note_targeted_reads_duplicate_and_quantize(self):
        song = FakeSong(); clip = FakeClip(4.0)
        clip.add_new_notes([{"pitch": 60, "start_time": 0.0, "duration": 0.5, "velocity": 100}, {"pitch": 64, "start_time": 0.6, "duration": 0.5, "velocity": 90}])
        clip.get_notes_by_id = lambda ids: [note for note in clip.notes if note["note_id"] in set(ids)]
        clip.get_selected_notes = lambda: [clip.notes[0]]
        def duplicate(ids):
            for note in [n for n in clip.notes if n["note_id"] in set(ids)]:
                copy = dict(note); copy["note_id"] = clip.next_note_id; clip.next_note_id += 1; clip.notes.append(copy)
        clip.duplicate_notes_by_id = duplicate
        def quantize(grid_enum, amount):
            # Live's RecordingQuantization number for 1/16 (no Live module here, so the bridge passes the number).
            if grid_enum != 5: raise TypeError("quantize takes a RecordingQuantization enum")
            grid = 0.25
            for note in clip.notes: note["start_time"] = round(note["start_time"] / grid) * grid * amount + note["start_time"] * (1 - amount)
        clip.quantize = quantize
        def quantize_pitch(pitch, grid_enum, amount):
            if grid_enum != 5: raise TypeError("quantize_pitch takes a RecordingQuantization enum")
            grid = 0.25
            for note in clip.notes:
                if note["pitch"] == pitch: note["start_time"] = round(note["start_time"] / grid) * grid
        clip.quantize_pitch = quantize_pitch
        song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["clips"][0]
        read = mapper.invoke("note.read-by-id", {"ref": row["ref"], "noteIds": [1]})
        self.assertEqual([note["pitch"] for note in read["notes"]], [60]); validate_operation_payload("note.read-by-id", "result", read)
        selected = mapper.invoke("note.read-selected", {"ref": row["ref"]})
        self.assertTrue(selected["available"]); self.assertEqual(len(selected["notes"]), 1)
        def note_fences():
            return {"ref": row["ref"], "expectedClipAuthority": mapper._session_clip_authority(row["ref"]), "expectedNotesRevision": hashlib.sha256(mapper._bounded_canonical(mapper._read_notes(clip)).encode()).hexdigest()}
        duplicated = mapper.invoke("note.duplicate", {**note_fences(), "noteIds": [1]})
        self.assertEqual(duplicated["duplicated"], 1); self.assertEqual(len(clip.notes), 3)
        with self.assertRaisesRegex(ValueError, "stable note identity"): mapper.invoke("note.duplicate", {**note_fences(), "noteIds": [99]})
        changed = mapper.invoke("note.quantize", {**note_fences(), "grid": 0.25, "amount": 1.0})
        self.assertTrue(changed["changed"]); self.assertEqual(clip.notes[1]["start_time"], 0.5)
        changed = mapper.invoke("note.quantize", {**note_fences(), "grid": 0.25, "amount": 1.0, "pitch": 64})
        self.assertTrue(changed["changed"]); self.assertEqual(clip.notes[1]["start_time"], 0.5); self.assertEqual(clip.notes[0]["start_time"], 0.0)
        with self.assertRaisesRegex(ValueError, "supported quantization grid"): mapper.invoke("note.quantize", {**note_fences(), "grid": 0.3, "amount": 1.0})


class FakeTakeLane:
    def __init__(self, name="Take 1"):
        self.name = name
        self.arrangement_clips = []

    def create_midi_clip(self, position, length):
        clip = FakeClip(length); clip.start_time = position; clip.is_take_lane_clip = True; self.arrangement_clips.append(clip); return clip

    def create_audio_clip(self, file_path, position):
        clip = FakeClip(4.0); clip.is_audio_clip = True; clip.start_time = position; clip.file_path = file_path; clip.is_take_lane_clip = True; self.arrangement_clips.append(clip); return clip


class TakeLaneExpansionTests(unittest.TestCase):
    def _mapper_with_lanes(self):
        song = FakeSong(); track = song.tracks[0]
        lane = FakeTakeLane()
        existing = FakeClip(4.0); existing.name = "Comp A"; existing.start_time = 0.0; existing.is_take_lane_clip = True
        lane.arrangement_clips = [existing]
        track.take_lanes = [lane]
        track.create_take_lane = lambda: (track.take_lanes.append(FakeTakeLane(f"Take {len(track.take_lanes) + 1}")) or track.take_lanes[-1])
        mapper = LiveObjectMapper(song)
        return song, track, lane, existing, mapper

    def test_take_lane_discovery_rows_and_read(self):
        _, track, lane, existing, mapper = self._mapper_with_lanes()
        track_row = mapper.snapshot()["tracks"][0]
        lanes = track_row["takeLanes"]
        self.assertEqual(len(lanes), 1); self.assertEqual(lanes[0]["name"], "Take 1"); self.assertEqual(lanes[0]["index"], 0)
        clip_row = lanes[0]["clips"][0]
        self.assertEqual(clip_row["name"], "Comp A"); self.assertTrue(clip_row["isTakeLaneClip"]); self.assertEqual(clip_row["takeLaneRef"], lanes[0]["ref"])
        self.assertEqual(mapper.get(lanes[0]["ref"])["name"], "Take 1")
        self.assertEqual(mapper.get(clip_row["ref"])["name"], "Comp A")
        self.assertIsNone(mapper.snapshot()["tracks"][0]["clips"] and mapper.snapshot()["tracks"][0]["clips"] == [] or None)
        result = mapper.invoke("audio.take-lane.read", {"trackRef": track_row["ref"]})
        self.assertEqual(result["lanes"], [{"ref": lanes[0]["ref"], "name": "Take 1"}]); validate_operation_payload("audio.take-lane.read", "result", result)
        self.assertTrue(mapper._operation_supported("audio.take-lane.read")); self.assertIn("takes", mapper.capabilities())

    def test_take_lane_create_with_collection_fencing(self):
        song, track, lane, existing, mapper = self._mapper_with_lanes()
        track_row = mapper.snapshot()["tracks"][0]
        args = {"trackRef": track_row["ref"], "name": "Take 2", "expectedTrackIdentity": track_row["objectIdentity"], "expectedTakeLaneCollectionRevision": mapper._take_lane_collection_revision(track, 0)}
        self.assertTrue(mapper._operation_supported("take-lane.create"))
        result = mapper.invoke("take-lane.create", args)
        self.assertEqual((result["name"], result["index"]), ("Take 2", 1)); validate_operation_payload("take-lane.create", "result", result)
        self.assertEqual(len(track.take_lanes), 2)
        with self.assertRaisesRegex(ValueError, "collection changed"): mapper.invoke("take-lane.create", args)

    def test_take_lane_create_confirms_by_identity_diff_when_creator_returns_none(self):
        song, track, lane, existing, mapper = self._mapper_with_lanes()
        # list.append returns None, matching Live shapes whose creator has no
        # documented return value; confirmation must come from the identity-diff.
        track.create_take_lane = lambda: track.take_lanes.append(FakeTakeLane(f"Take {len(track.take_lanes) + 1}"))
        track_row = mapper.snapshot()["tracks"][0]
        args = {"trackRef": track_row["ref"], "name": "Take 2", "expectedTrackIdentity": track_row["objectIdentity"], "expectedTakeLaneCollectionRevision": mapper._take_lane_collection_revision(track, 0)}
        result = mapper.invoke("take-lane.create", args)
        self.assertEqual((result["name"], result["index"]), ("Take 2", 1)); validate_operation_payload("take-lane.create", "result", result)
        self.assertEqual(len(track.take_lanes), 2)

    def test_take_lane_clip_create_confirms_by_identity_diff_when_creator_returns_none(self):
        _, track, lane, existing, mapper = self._mapper_with_lanes()
        lane_row = mapper.snapshot()["tracks"][0]["takeLanes"][0]
        def create_midi_clip(position, length):
            clip = FakeClip(length); clip.start_time = position; clip.is_take_lane_clip = True
            lane.arrangement_clips.append(clip)  # append returns None: the undocumented-return shape
        def create_audio_clip(file_path, position):
            clip = FakeClip(4.0); clip.is_audio_clip = True; clip.start_time = position; clip.file_path = file_path; clip.is_take_lane_clip = True
            lane.arrangement_clips.append(clip)
        lane.create_midi_clip = create_midi_clip; lane.create_audio_clip = create_audio_clip
        base = {"takeLaneRef": lane_row["ref"], "expectedTakeLaneIdentity": lane_row["objectIdentity"], "expectedCollectionRevision": mapper._take_lane_clip_collection_revision(lane, lane_row["ref"])}
        result = mapper.invoke("take-lane.clip.create", {**base, "position": 8.0, "length": 4.0, "name": "New Take"})
        self.assertEqual((result["name"], result["start"], result["length"]), ("New Take", 8.0, 4.0)); validate_operation_payload("take-lane.clip.create", "result", result)
        audio = mapper.invoke("take-lane.audio-clip.create", {**base, "expectedCollectionRevision": mapper._take_lane_clip_collection_revision(lane, lane_row["ref"]), "filePath": "/tmp/demo.wav", "position": 16.0, "name": "Audio Take"})
        self.assertEqual((audio["filePath"], audio["start"]), ("/tmp/demo.wav", 16.0)); validate_operation_payload("take-lane.audio-clip.create", "result", audio)
        self.assertEqual(len(lane.arrangement_clips), 3)

    def test_take_lane_rename_with_rollback(self):
        _, track, lane, existing, mapper = self._mapper_with_lanes()
        lane_row = mapper.snapshot()["tracks"][0]["takeLanes"][0]
        args = {"ref": lane_row["ref"], "name": "Verse Take", "expectedName": "Take 1", "expectedObjectIdentity": lane_row["objectIdentity"], "expectedAuthorityRevision": mapper._take_lane_collection_revision(track, 0)}
        result = mapper.invoke("take-lane.rename", args)
        self.assertEqual(result, {"renamed": lane_row["ref"], "name": "Verse Take"}); self.assertEqual(lane.name, "Verse Take")
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("take-lane.rename", args)
        class FailingLane(FakeTakeLane):
            @property
            def name(self): return self._name
            @name.setter
            def name(self, value):
                if value == "Boom": raise RuntimeError("rename rejected")
                self._name = value
        failing = FailingLane(); failing._name = "Old"; track.take_lanes = [failing]
        mapper2 = LiveObjectMapper(_song_with_lanes(track_holder=[track])) if False else LiveObjectMapper(FakeSong())
        song2 = FakeSong(); song2.tracks[0] = track; mapper2 = LiveObjectMapper(song2)
        failing_row = mapper2.snapshot()["tracks"][0]["takeLanes"][0]
        bad_args = {"ref": failing_row["ref"], "name": "Boom", "expectedName": "Old", "expectedObjectIdentity": failing_row["objectIdentity"], "expectedAuthorityRevision": mapper2._take_lane_collection_revision(track, 0)}
        with self.assertRaisesRegex(ValueError, "postcondition was not confirmed"): mapper2.invoke("take-lane.rename", bad_args)
        self.assertEqual(failing.name, "Old")

    def test_take_lane_rename_undo_round_trip(self):
        _, track, lane, existing, mapper = self._mapper_with_lanes()
        lane_row = mapper.snapshot()["tracks"][0]["takeLanes"][0]
        apply_args = {"ref": lane_row["ref"], "name": "Verse Take", "expectedName": "Take 1", "expectedObjectIdentity": lane_row["objectIdentity"], "expectedAuthorityRevision": mapper._take_lane_collection_revision(track, 0)}
        applied = mapper.invoke("take-lane.rename", apply_args)
        self.assertEqual(applied, {"renamed": lane_row["ref"], "name": "Verse Take"}); validate_operation_payload("take-lane.rename", "result", applied)
        undo_args = {"ref": lane_row["ref"], "name": "Take 1", "expectedName": "Verse Take", "expectedObjectIdentity": lane_row["objectIdentity"], "expectedAuthorityRevision": mapper._take_lane_collection_revision(track, 0)}
        undone = mapper.invoke("take-lane.rename", undo_args)
        self.assertEqual(undone, {"renamed": lane_row["ref"], "name": "Take 1"}); validate_operation_payload("take-lane.rename", "result", undone); self.assertEqual(lane.name, "Take 1")

    def test_take_lane_clip_create_midi_and_audio(self):
        _, track, lane, existing, mapper = self._mapper_with_lanes()
        lane_row = mapper.snapshot()["tracks"][0]["takeLanes"][0]
        base = {"takeLaneRef": lane_row["ref"], "expectedTakeLaneIdentity": lane_row["objectIdentity"], "expectedCollectionRevision": mapper._take_lane_clip_collection_revision(lane, lane_row["ref"])}
        result = mapper.invoke("take-lane.clip.create", {**base, "position": 8.0, "length": 4.0, "name": "New Take"})
        self.assertEqual((result["name"], result["start"], result["length"]), ("New Take", 8.0, 4.0)); validate_operation_payload("take-lane.clip.create", "result", result)
        self.assertEqual(len(lane.arrangement_clips), 2); self.assertTrue(lane.arrangement_clips[1].is_take_lane_clip)
        audio = mapper.invoke("take-lane.audio-clip.create", {"takeLaneRef": lane_row["ref"], "expectedTakeLaneIdentity": lane_row["objectIdentity"], "expectedCollectionRevision": mapper._take_lane_clip_collection_revision(lane, lane_row["ref"]), "filePath": "/tmp/demo.wav", "position": 16.0, "name": "Audio Take"})
        self.assertEqual((audio["filePath"], audio["start"]), ("/tmp/demo.wav", 16.0)); validate_operation_payload("take-lane.audio-clip.create", "result", audio)
        with self.assertRaisesRegex(ValueError, "absolute path"): mapper.invoke("take-lane.audio-clip.create", {**base, "filePath": "demo.wav", "position": 20.0})
        clip_row = mapper.snapshot()["tracks"][0]["takeLanes"][0]["clips"][1]
        self.assertTrue(clip_row["isTakeLaneClip"])
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        current = mapper.get(clip_row["ref"])
        args = {"ref": clip_row["ref"], "muted": True, "expectedObjectIdentity": clip_row["objectIdentity"], "expectedAuthorityRevision": mapper._clip_authority_digest(clip_row["ref"]), "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        lane.arrangement_clips[1].muted = False; lane.arrangement_clips[1].color_index = 1; lane.arrangement_clips[1].looping = True; lane.arrangement_clips[1].loop_start = 0.0; lane.arrangement_clips[1].loop_end = 4.0
        current = mapper.get(clip_row["ref"]); args["expectedStateRevision"] = hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()
        result = mapper.invoke("clip.set", args)
        self.assertTrue(result["changed"]); self.assertTrue(lane.arrangement_clips[1].muted)


class FakeTuningSystem:
    def __init__(self):
        self.name = "Equal"
        self.lowest_note = {"note": 0, "deviation": 0.0}
        self.highest_note = {"note": 127, "deviation": 0.0}
        self.reference_pitch = {"note": 69, "frequency": 440.0}
        self.pseudo_octave_in_cents = 1200.0
        self.note_tunings = [{"note": index, "deviation": 0.0} for index in range(128)]


class TuningScaleTests(unittest.TestCase):
    def _mapper_with_tuning(self):
        song = FakeSong()
        song.tuning_system = FakeTuningSystem()
        song.root_note = 0; song.scale_name = "Major"; song.scale_mode = True; song.scale_intervals = [0, 2, 4, 5, 7, 9, 11]
        return song, mapper if False else LiveObjectMapper(song)

    def test_tuning_read_exposes_system_and_scale(self):
        song, mapper = self._mapper_with_tuning()
        self.assertTrue(mapper._operation_supported("tuning.read")); self.assertTrue(mapper._operation_supported("tuning.set"))
        set_ref = mapper.snapshot()["set"]["ref"]
        result = mapper.invoke("tuning.read", {"setRef": set_ref})
        self.assertEqual(result["tuningSystem"]["name"], "Equal"); self.assertEqual(result["tuningSystem"]["referencePitch"], {"note": 69, "frequency": 440.0})
        self.assertEqual(result["tuningSystem"]["pseudoOctaveInCents"], 1200.0); self.assertEqual(len(result["tuningSystem"]["noteTunings"]), 128)
        self.assertEqual(result["scale"], {"rootNote": 0, "scaleName": "Major", "scaleMode": True, "scaleIntervals": [0, 2, 4, 5, 7, 9, 11]})
        validate_operation_payload("tuning.read", "result", result)

    def test_tuning_set_validates_and_rolls_back_exactly(self):
        song, mapper = self._mapper_with_tuning()
        set_ref = mapper.snapshot()["set"]["ref"]; identity = mapper.snapshot()["set"]["objectIdentity"]
        def fences(): return {"setRef": set_ref, "expectedObjectIdentity": identity, "expectedRevision": mapper._tuning_revision()}
        result = mapper.invoke("tuning.set", {**fences(), "referencePitch": {"note": 69, "frequency": 432.0}, "rootNote": 9, "scaleName": "Minor", "scaleMode": False})
        self.assertTrue(result["changed"]); validate_operation_payload("tuning.set", "result", result)
        self.assertEqual(song.tuning_system.reference_pitch, {"note": 69, "frequency": 432.0}); self.assertEqual(song.root_note, 9); self.assertEqual(song.scale_name, "Minor"); self.assertEqual(song.scale_mode, False)
        stale = fences(); stale["expectedRevision"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("tuning.set", {**stale, "rootNote": 0})
        with self.assertRaisesRegex(ValueError, "referencePitch is invalid"): mapper.invoke("tuning.set", {**fences(), "referencePitch": 432.0})
        with self.assertRaisesRegex(ValueError, "tuning fields are invalid"): mapper.invoke("tuning.set", {**fences(), "scaleIntervals": [0, 2, 3]})
        with self.assertRaisesRegex(ValueError, "scaleMode is invalid"): mapper.invoke("tuning.set", {**fences(), "scaleMode": "Ionian"})
        with self.assertRaisesRegex(ValueError, "exactly 128"): mapper.invoke("tuning.set", {**fences(), "noteTunings": [{"note": 0, "deviation": 0.0}]})
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("tuning.set", fences())
        rows = [{"note": index, "deviation": 5.0 if index == 69 else 0.0} for index in range(128)]
        result = mapper.invoke("tuning.set", {**fences(), "noteTunings": rows})
        self.assertTrue(result["changed"]); self.assertEqual(song.tuning_system.note_tunings[69]["deviation"], 5.0)
        class FailingTuning(FakeTuningSystem):
            @property
            def reference_pitch(self): return self._pitch
            @reference_pitch.setter
            def reference_pitch(self, value):
                if value == {"note": 69, "frequency": 415.0}: raise RuntimeError("tuning rejected")
                self._pitch = value
        failing = FailingTuning(); failing._pitch = {"note": 69, "frequency": 440.0}; song.tuning_system = failing
        with self.assertRaisesRegex(RuntimeError, "tuning rejected"):
            mapper.invoke("tuning.set", {**fences(), "referencePitch": {"note": 69, "frequency": 415.0}, "rootNote": 2})
        self.assertEqual(failing.reference_pitch, {"note": 69, "frequency": 440.0}); self.assertEqual(song.root_note, 9)


class FakeGroove:
    def __init__(self, name="Swing 16"):
        self.name = name
        self.base = 3
        self.quantization_amount = 0.5
        self.random_amount = 0.1
        self.timing_amount = 0.6
        self.velocity_amount = 0.2


class FakeGroovePool:
    def __init__(self, grooves=None):
        self.grooves = grooves if grooves is not None else [FakeGroove()]


class GroovePoolTests(unittest.TestCase):
    def _mapper_with_groove(self):
        song = FakeSong()
        song.groove_pool = FakeGroovePool()
        song.groove_amount = 0.0
        return song, LiveObjectMapper(song)

    def test_groove_read_exposes_pool_and_amount(self):
        song, mapper = self._mapper_with_groove()
        self.assertTrue(mapper._operation_supported("groove.read")); self.assertTrue(mapper._operation_supported("groove.set")); self.assertTrue(mapper._operation_supported("groove.edit"))
        set_ref = mapper.snapshot()["set"]["ref"]
        result = mapper.invoke("groove.read", {"setRef": set_ref})
        self.assertEqual(result["grooveAmount"], 0.0); self.assertEqual(len(result["grooves"]), 1)
        row = result["grooves"][0]
        self.assertEqual((row["name"], row["base"], row["quantizationAmount"], row["timingAmount"]), ("Swing 16", 3, 0.5, 0.6))
        validate_operation_payload("groove.read", "result", result)

    def test_groove_set_amount_with_rollback(self):
        song, mapper = self._mapper_with_groove()
        set_ref = mapper.snapshot()["set"]["ref"]; identity = mapper.snapshot()["set"]["objectIdentity"]
        def fences(): return {"setRef": set_ref, "expectedObjectIdentity": identity, "expectedRevision": mapper._groove_revision()}
        result = mapper.invoke("groove.set", {**fences(), "grooveAmount": 0.75})
        self.assertTrue(result["changed"]); validate_operation_payload("groove.set", "result", result); self.assertEqual(song.groove_amount, 0.75)
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("groove.set", {**fences(), "grooveAmount": 2.0})
        stale = fences(); stale["expectedRevision"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("groove.set", {**stale, "grooveAmount": 0.5})

    def test_groove_edit_fields_with_rollback(self):
        song, mapper = self._mapper_with_groove()
        set_ref = mapper.snapshot()["set"]["ref"]; identity = mapper.snapshot()["set"]["objectIdentity"]
        groove_ref = mapper.invoke("groove.read", {"setRef": set_ref})["grooves"][0]["ref"]
        groove = song.groove_pool.grooves[0]
        object_identity = mapper._capture_object_identity(groove)
        def fences(): return {"ref": groove_ref, "expectedObjectIdentity": object_identity, "expectedRevision": mapper._groove_revision()}
        result = mapper.invoke("groove.edit", {**fences(), "name": "MPC 57", "timingAmount": 0.57, "velocityAmount": 0.3})
        self.assertTrue(result["changed"]); validate_operation_payload("groove.edit", "result", result)
        self.assertEqual((groove.name, groove.timing_amount, groove.velocity_amount), ("MPC 57", 0.57, 0.3))
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("groove.edit", {**fences(), "timingAmount": 150.0})
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("groove.edit", {**fences(), "velocityAmount": -101.0})
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("groove.edit", fences())

    def test_grooves_read_and_edit_in_the_units_live_reports(self):
        # Live's Groove Pool shows (and its API reports) percentages, with velocity from -100 to 100, and
        # keeps them as 32-bit floats; the Set's groove amount goes to 131.25%. A groove read with
        # Timing 100% must pass the registry, or the groove preview fails its response contract.
        class Base(int): pass  # Live's groove base is an enum: an int subclass
        class Float32Groove(FakeGroove):
            def __setattr__(self, name, value): object.__setattr__(self, name, float32(value) if name.endswith("_amount") else value)
        groove = Float32Groove("Swing 16ths 66"); groove.base = Base(3); groove.quantization_amount = 0.0; groove.random_amount = 0.0; groove.timing_amount = 100.0; groove.velocity_amount = -30.0
        song, mapper = self._mapper_with_groove(); song.groove_pool = FakeGroovePool([groove]); song.groove_amount = 1.3125
        set_ref = mapper.snapshot()["set"]["ref"]
        read = mapper.invoke("groove.read", {"setRef": set_ref}); validate_operation_payload("groove.read", "result", read)
        self.assertEqual((read["grooves"][0]["timingAmount"], read["grooves"][0]["velocityAmount"], read["grooveAmount"]), (100.0, -30.0, 1.3125))
        edit = {"ref": read["grooves"][0]["ref"], "timingAmount": 66.6, "velocityAmount": -12.5, "expectedObjectIdentity": mapper._capture_object_identity(groove), "expectedRevision": mapper._groove_revision()}
        validate_operation_payload("groove.edit", "request", edit)
        self.assertTrue(mapper.invoke("groove.edit", edit)["changed"])
        self.assertNotEqual(groove.timing_amount, 66.6, "the fake keeps 32-bit floats like Live"); self.assertAlmostEqual(groove.timing_amount, 66.6, places=4)
        validate_operation_payload("groove.set", "request", {"setRef": set_ref, "grooveAmount": 1.3125, "expectedObjectIdentity": mapper.snapshot()["set"]["objectIdentity"], "expectedRevision": mapper._groove_revision()})

    def test_clip_groove_assignment_and_has_groove(self):
        song, mapper = self._mapper_with_groove()
        clip = FakeClip(4.0); clip.is_audio_clip = False; clip.muted = False; clip.color_index = 0; clip.looping = True; clip.loop_start = 0.0; clip.loop_end = 4.0; clip.groove = None
        song.tracks[0].clip_slots[0].clip = clip
        groove = song.groove_pool.grooves[0]
        set_ref = mapper.snapshot()["set"]["ref"]
        groove_ref = mapper.invoke("groove.read", {"setRef": set_ref})["grooves"][0]["ref"]
        row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertIsNone(row["groove"]); self.assertFalse(row["hasGroove"])
        fields = LiveObjectMapper._CLIP_SET_FIELDS
        def fences():
            current = mapper.get(row["ref"])
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._clip_authority_digest(row["ref"]),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({field: current.get(field) for field in fields}).encode()).hexdigest()}
        result = mapper.invoke("clip.set", {**fences(), "grooveRef": groove_ref})
        self.assertTrue(result["changed"]); self.assertIs(clip.groove, groove)
        row = mapper.snapshot()["tracks"][0]["clips"][0]
        self.assertEqual(row["groove"]["name"], "Swing 16"); self.assertTrue(row["hasGroove"])
        result = mapper.invoke("clip.set", {**fences(), "grooveRef": None})
        self.assertTrue(result["changed"]); self.assertIsNone(clip.groove)
        with self.assertRaisesRegex(ValueError, "grooveRef is invalid"): mapper.invoke("clip.set", {**fences(), "grooveRef": "bogus"})


class SceneSlotExpansionTests(unittest.TestCase):
    def _mapper_with_scene(self):
        song = FakeSong()
        scene = song.scenes[0]
        scene.color_index = 1; scene.is_empty = False; scene.is_triggered = False
        scene.tempo = 120.0; scene.tempo_enabled = False
        scene.time_signature_numerator = 4; scene.time_signature_denominator = 4; scene.time_signature_enabled = False
        slot = song.tracks[0].clip_slots[0]
        slot.color_index = 2; slot.controls_other_clips = False; slot.has_stop_button = True; slot.is_group_slot = False; slot.playing_status = 0; slot.will_record_on_start = False
        return song, scene, slot, LiveObjectMapper(song)

    def test_scene_and_slot_rows_expose_state(self):
        _, scene, slot, mapper = self._mapper_with_scene()
        row = mapper.snapshot()["scenes"][0]
        self.assertEqual((row["colorIndex"], row["isEmpty"], row["isTriggered"], row["tempo"], row["tempoEnabled"]), (1, False, False, 120.0, False))
        self.assertEqual((row["signatureNumerator"], row["signatureDenominator"], row["timeSignatureEnabled"]), (4, 4, False))
        slot_row = mapper.snapshot()["tracks"][0]["clipSlots"][0]
        self.assertEqual((slot_row["colorIndex"], slot_row["controlsOtherClips"], slot_row["hasStopButton"], slot_row["isGroupSlot"], slot_row["playingStatus"], slot_row["willRecordOnStart"]), (2, False, True, False, 0, False))

    def test_scene_set_with_validation_and_rollback(self):
        song, scene, slot, mapper = self._mapper_with_scene()
        row = mapper.snapshot()["scenes"][0]
        def fences():
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._scene_collection_revision(),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._scene_state_fields(scene)).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("scene.set"))
        result = mapper.invoke("scene.set", {**fences(), "colorIndex": 5, "tempo": 90.0, "tempoEnabled": True, "signatureNumerator": 6, "signatureDenominator": 8, "timeSignatureEnabled": True})
        self.assertTrue(result["changed"]); validate_operation_payload("scene.set", "result", result)
        self.assertEqual((scene.color_index, scene.tempo, scene.tempo_enabled), (5, 90.0, True))
        self.assertEqual((scene.time_signature_numerator, scene.time_signature_denominator, scene.time_signature_enabled), (6, 8, True))
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("scene.set", {**fences(), "tempo": 10000.0})
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("scene.set", fences())
        stale = fences(); stale["expectedStateRevision"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("scene.set", {**stale, "colorIndex": 3})
        class FailingScene(FakeScene):
            @property
            def tempo(self): return self._tempo
            @tempo.setter
            def tempo(self, value):
                if value == 60.0: raise RuntimeError("tempo rejected")
                self._tempo = value
        failing = FailingScene(); failing._tempo = 120.0; failing.color_index = 1
        failing.time_signature_numerator = 4; failing.time_signature_denominator = 4; failing.time_signature_enabled = False; failing.tempo_enabled = False
        song.scenes = [failing]; mapper2 = LiveObjectMapper(song)
        failing_row = mapper2.snapshot()["scenes"][0]
        bad_args = {"ref": failing_row["ref"], "expectedObjectIdentity": failing_row["objectIdentity"], "expectedAuthorityRevision": mapper2._scene_collection_revision(),
                    "expectedStateRevision": hashlib.sha256(mapper2._bounded_canonical(mapper2._scene_state_fields(failing)).encode()).hexdigest(), "tempo": 60.0, "colorIndex": 9}
        with self.assertRaisesRegex(RuntimeError, "tempo rejected"): mapper2.invoke("scene.set", bad_args)
        self.assertEqual(failing.tempo, 120.0); self.assertEqual(failing.color_index, 1)

    def test_a_switched_off_scene_tempo_reads_minus_one_and_is_restored_by_its_switch(self):
        class SwitchedScene(FakeScene):
            """Like Live: a scene's tempo reads -1 while its tempo is switched off, and -1 can't be written."""
            def __init__(self, name="Scene 1", refuse_enable=False): super().__init__(name); self._tempo = 120.0; self.refuse_enable = refuse_enable; self._enabled = False; self.color_index = 1; self.time_signature_numerator = 4; self.time_signature_denominator = 4; self.time_signature_enabled = False
            @property
            def tempo(self): return self._tempo if self._enabled else -1.0
            @tempo.setter
            def tempo(self, value):
                if not 20 <= value <= 999: raise RuntimeError("Invalid tempo")
                self._tempo = value
            @property
            def tempo_enabled(self): return self._enabled
            @tempo_enabled.setter
            def tempo_enabled(self, value): self._enabled = bool(value) and not self.refuse_enable
        song = FakeSong(); scene = SwitchedScene(); song.scenes = [scene]; mapper = LiveObjectMapper(song)
        def authority():
            row = mapper.snapshot()["scenes"][0]
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._scene_collection_revision(), "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._scene_state_fields(scene)).encode()).hexdigest()}
        before = mapper.snapshot()["scenes"][0]; self.assertEqual((before["tempo"], before["tempoEnabled"]), (-1.0, False))
        self.assertTrue(mapper.invoke("scene.set", {**authority(), "tempo": 122.0, "tempoEnabled": True})["changed"])
        self.assertEqual((scene.tempo, scene.tempo_enabled), (122.0, True))
        # The prior -1 is outside what can be written: restoring it means switching the tempo off.
        with self.assertRaises(ValueError): validate_operation_payload("scene.set", "request", {**authority(), "tempo": -1.0, "tempoEnabled": False})
        restore = {**authority(), "tempoEnabled": False}; validate_operation_payload("scene.set", "request", restore)
        self.assertTrue(mapper.invoke("scene.set", restore)["changed"])
        after = mapper.snapshot()["scenes"][0]; self.assertEqual((after["tempo"], after["tempoEnabled"]), (before["tempo"], before["tempoEnabled"]))
        # A failed change rolls back to the switched-off tempo by its switch, never by writing -1.
        refusing = SwitchedScene("Scene 2", refuse_enable=True); song.scenes = [refusing]; mapper = LiveObjectMapper(song); scene = refusing
        with self.assertRaisesRegex(ValueError, "scene change was not confirmed"): mapper.invoke("scene.set", {**authority(), "tempo": 122.0, "tempoEnabled": True})
        self.assertEqual((refusing.tempo, refusing.tempo_enabled), (-1.0, False))

    def test_scene_fire_selected_is_accepted_before_live_applies_it(self):
        song, scene, slot, mapper = self._mapper_with_scene()
        # Live 12.4 launches the scene on its next tick: nothing reads as queued or playing right after.
        pending = []
        def fire_as_selected():
            pending.append(lambda: (setattr(scene, "is_triggered", True), setattr(song, "is_playing", True)))
        scene.fire_as_selected = fire_as_selected
        row = mapper.snapshot()["scenes"][0]
        self.assertTrue(mapper._operation_supported("scene.fire-selected"))
        playback = mapper._playback()
        state = hashlib.sha256(mapper._bounded_canonical({"isTriggered": False, "playing": playback["transport"]["playing"]}).encode()).hexdigest()
        result = mapper.invoke("scene.fire-selected", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._scene_collection_revision(), "expectedStateRevision": state})
        self.assertEqual(result, {"fired": True}); validate_operation_payload("scene.fire-selected", "result", result)
        self.assertEqual(len(pending), 1, "fired once"); pending[0]()
        self.assertTrue(scene.is_triggered); self.assertTrue(song.is_playing)


class SongTransportLinkTests(unittest.TestCase):
    def _mapper_with_song_state(self):
        song = FakeSong()
        song.visible_tracks = list(song.tracks); song.appointed_device = song.tracks[0].devices[0]
        song.song_length = 64.0; song.start_time = 0.0
        song.signature_numerator = 4; song.signature_denominator = 4; song.swing_amount = 0.0
        song.overdub = False; song.arrangement_overdub = False; song.back_to_arranger = False
        song.can_capture_midi = True; song.can_undo = True; song.can_redo = False
        song.exclusive_arm = True; song.exclusive_solo = True; song.is_counting_in = False
        song.tempo_follower_enabled = False; song.re_enable_automation_enabled = False
        song.session_automation_record = False
        song.is_ableton_link_enabled = True; song.is_ableton_link_start_stop_sync_enabled = False
        song.tempo = 120.0
        class FakeQuantizationEnum:
            name = "grid_sixteenth"
            def __int__(self): return 5
        song.clip_trigger_quantization = FakeQuantizationEnum()
        return song, LiveObjectMapper(song)

    def test_song_read_exposes_state(self):
        song, mapper = self._mapper_with_song_state()
        set_ref = mapper.snapshot()["set"]["ref"]
        result = mapper.invoke("song.read", {"setRef": set_ref})
        self.assertEqual(len(result["visibleTracks"]), 1); self.assertIsNotNone(result["appointedDevice"])
        self.assertEqual((result["songLength"], result["signatureNumerator"], result["swingAmount"]), (64.0, 4, 0.0))
        self.assertEqual((result["canCaptureMidi"], result["canUndo"], result["canRedo"]), (True, True, False))
        self.assertEqual((result["exclusiveArm"], result["exclusiveSolo"], result["isCountingIn"]), (True, True, False))
        self.assertEqual((result["isAbletonLinkEnabled"], result["isAbletonLinkStartStopSyncEnabled"]), (True, False))
        self.assertEqual(result["clipTriggerQuantization"], {"name": "grid_sixteenth", "value": 5})
        validate_operation_payload("song.read", "result", result)

    def test_song_set_writes_playback_settings_with_exact_rollback(self):
        song, mapper = self._mapper_with_song_state()
        song.midi_recording_quantization = 0
        self.assertTrue(mapper._operation_supported("song.set"))
        def authority(target=mapper):
            row = target.snapshot()["set"]
            return {"setRef": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(target._bounded_canonical(target._song_settings_state()).encode()).hexdigest()}
        valid = {**authority(), "signatureNumerator": 6, "signatureDenominator": 8}
        validate_operation_payload("song.set", "request", valid)
        result = mapper.invoke("song.set", valid)
        self.assertTrue(result["changed"]); validate_operation_payload("song.set", "result", result)
        self.assertEqual((song.signature_numerator, song.signature_denominator), (6, 8))
        result = mapper.invoke("song.set", {**authority(), "swingAmount": 0.5, "clipTriggerQuantization": 7, "midiRecordingQuantization": 5})
        self.assertTrue(result["changed"])
        self.assertEqual((song.swing_amount, song.clip_trigger_quantization, song.midi_recording_quantization), (0.5, 7, 5))
        with self.assertRaisesRegex(ValueError, "signatureNumerator is invalid"): mapper.invoke("song.set", {**authority(), "signatureNumerator": 0})
        with self.assertRaisesRegex(ValueError, "swingAmount is invalid"): mapper.invoke("song.set", {**authority(), "swingAmount": 1.5})
        with self.assertRaisesRegex(ValueError, "clipTriggerQuantization is invalid"): mapper.invoke("song.set", {**authority(), "clipTriggerQuantization": 14})
        with self.assertRaisesRegex(ValueError, "midiRecordingQuantization is invalid"): mapper.invoke("song.set", {**authority(), "midiRecordingQuantization": 9})
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("song.set", {**authority(), "swingAmount": 0.25, "expectedStateRevision": "0" * 64})
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("song.set", authority())
        with self.assertRaisesRegex(ValueError, "Set identity changed"): mapper.invoke("song.set", {**authority(), "swingAmount": 0.25, "expectedObjectIdentity": "replacement"})
        with self.assertRaisesRegex(ValueError, "Set identity changed"): mapper.invoke("song.set", {"swingAmount": 0.25, "expectedStateRevision": authority()["expectedStateRevision"]})
        self.assertEqual(song.swing_amount, 0.5)
        class SignatureRefusingSong(FakeSong):
            @property
            def signature_denominator(self): return self._denominator
            @signature_denominator.setter
            def signature_denominator(self, value):
                if value != self._denominator: raise RuntimeError("denominator write rejected")
                self._denominator = value
        refusing = SignatureRefusingSong()
        refusing.signature_numerator = 3; refusing._denominator = 4; refusing.swing_amount = 0.0
        refusing.clip_trigger_quantization = 4; refusing.midi_recording_quantization = 0
        mapper2 = LiveObjectMapper(refusing)
        with self.assertRaises(RuntimeError):
            mapper2.invoke("song.set", {**authority(mapper2), "signatureNumerator": 7, "signatureDenominator": 8})
        self.assertEqual((refusing.signature_numerator, refusing.signature_denominator), (3, 4))
        plain = LiveObjectMapper(FakeSong())
        self.assertFalse(plain._operation_supported("song.set"))
        with self.assertRaisesRegex(ValueError, "unavailable on this song"):
            plain.invoke("song.set", {**authority(plain), "swingAmount": 0.5})

    def test_transport_action_dispatches_and_fences(self):
        song, mapper = self._mapper_with_song_state()
        calls = []
        song.start_playing = lambda: calls.append("start")
        song.continue_playing = lambda: calls.append("continue")
        song.stop_playing = lambda: calls.append("stop")
        song.tap_tempo = lambda: calls.append("tap")
        song.nudge_up = False
        song.nudge_down = False
        song.re_enable_automation = lambda: calls.append("reenable")
        song.trigger_session_record = lambda: calls.append("record")
        song.force_link_beat_time = lambda beat: calls.append(("link", beat))
        song.stop_all_clips = lambda: calls.append("stop-all")
        song.scrub_by = lambda distance: calls.append(("scrub", distance))
        set_ref = mapper.snapshot()["set"]["ref"]; identity = mapper.snapshot()["set"]["objectIdentity"]
        self.assertTrue(mapper._operation_supported("transport.action"))
        def fences(): return {"setRef": set_ref, "expectedObjectIdentity": identity, "expectedRevision": str(mapper._playback()["revision"])}
        for action in ("start", "continue", "stop", "tap-tempo", "nudge-up", "nudge-down", "re-enable-automation", "trigger-session-record", "stop-all-clips"):
            # The fence is the Set's playback revision ("<epoch>:playback:<n>:<digest>"), as transport.set's is:
            # the registry must let it through on both sides, not only a 64-hex digest.
            request = {**fences(), "action": action}; self.assertRegex(request["expectedRevision"], r"^\d+:playback:\d+:[0-9a-f]{16}$")
            validate_operation_payload("transport.action", "request", request)
            result = mapper.invoke("transport.action", request)
            self.assertTrue(result["done"]); validate_operation_payload("transport.action", "result", result)
        self.assertEqual(calls, ["start", "continue", "stop", "tap", "reenable", "record", "stop-all"])
        self.assertEqual((song.nudge_up, song.nudge_down), (True, True))
        result = mapper.invoke("transport.action", {**fences(), "action": "scrub", "beatTime": 0.5})
        self.assertTrue(result["done"]); self.assertEqual(calls[-1], ("scrub", 0.5))
        with self.assertRaisesRegex(ValueError, "distance is required"): mapper.invoke("transport.action", {**fences(), "action": "scrub"})
        result = mapper.invoke("transport.action", {**fences(), "action": "force-link-beat-time", "beatTime": 8.0})
        self.assertTrue(result["done"]); self.assertEqual(calls[-1], ("link", 8.0))
        with self.assertRaisesRegex(ValueError, "beatTime is required"): mapper.invoke("transport.action", {**fences(), "action": "force-link-beat-time"})
        stale = fences(); stale["expectedRevision"] = "stale"
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("transport.action", {**stale, "action": "start"})
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("transport.action", {**fences(), "action": "detonate"})
        # Back to Arrangement: the lit button reads true, and writing false presses it.
        song.back_to_arranger = True
        request = {**fences(), "action": "back-to-arrangement"}; validate_operation_payload("transport.action", "request", request)
        result = mapper.invoke("transport.action", request)
        self.assertTrue(result["done"]); validate_operation_payload("transport.action", "result", result); self.assertIs(song.back_to_arranger, False)
        del song.back_to_arranger
        with self.assertRaisesRegex(ValueError, "back-to-arrangement is unavailable"): mapper.invoke("transport.action", {**fences(), "action": "back-to-arrangement"})

    def test_locator_jump_to_specific_cue(self):
        song = FakeArrangementSong()
        song.cue_points = [FakeLocator(4.0, "A"), FakeLocator(16.0, "B")]
        for locator in song.cue_points:
            locator.jump = lambda loc=locator: setattr(song, "current_song_time", loc.time)
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("locator.jump-to"))
        locators = mapper._locator_items()
        result = mapper.invoke("locator.jump-to", {"ref": locators[1]["ref"], "expectedObjectIdentity": locators[1]["objectIdentity"], "expectedCollectionRevision": hashlib.sha256(mapper._bounded_canonical(mapper._locator_items()).encode()).hexdigest()})
        self.assertEqual(result["position"], 16.0); validate_operation_payload("locator.jump-to", "result", result)
        without = LiveObjectMapper(FakeSong())
        self.assertFalse(without._operation_supported("locator.jump-to"))

    def test_locator_jump_to_confirms_within_tolerance_and_refuses_gross_mismatch(self):
        song = FakeArrangementSong()
        song.cue_points = [FakeLocator(4.0, "A"), FakeLocator(16.0, "B")]
        song.cue_points[0].jump = lambda: setattr(song, "current_song_time", 4.0 + 5e-4)
        song.cue_points[1].jump = lambda: setattr(song, "current_song_time", 16.5)
        mapper = LiveObjectMapper(song)
        locators = mapper._locator_items()
        def args_for(row):
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedCollectionRevision": hashlib.sha256(mapper._bounded_canonical(mapper._locator_items()).encode()).hexdigest()}
        result = mapper.invoke("locator.jump-to", args_for(locators[0]))
        self.assertAlmostEqual(result["position"], 4.0005, places=6); validate_operation_payload("locator.jump-to", "result", result)
        with self.assertRaisesRegex(ValueError, "locator jump was not confirmed"):
            mapper.invoke("locator.jump-to", args_for(locators[1]))

    def test_song_time_convert_documented_queries(self):
        song = FakeSong()
        song.get_beats_loop_start = lambda: 8.0
        song.get_beats_loop_length = lambda: 4.0
        class SmpteTime:
            def __init__(self): self.hours = 0; self.minutes = 1; self.seconds = 2; self.frames = 12; self.subframes = 3
        captured = {}
        def smpte(fmt): captured["format"] = fmt; return SmpteTime()
        song.get_current_smpte_song_time = smpte
        mapper = LiveObjectMapper(song); mapper._smpte_format_enum = lambda name: name
        set_ref = mapper.snapshot()["set"]["ref"]
        loop = mapper.invoke("song.time-convert", {"setRef": set_ref, "query": "beats-loop"})
        self.assertEqual(loop, {"available": True, "loopStart": 8.0, "loopLength": 4.0, "smpte": None})
        validate_operation_payload("song.time-convert", "result", loop)
        smpte_result = mapper.invoke("song.time-convert", {"setRef": set_ref, "query": "current-smpte", "smpteFormat": "smpte-30"})
        self.assertEqual(captured["format"], "smpte_30")
        self.assertEqual(smpte_result["smpte"], {"hours": 0, "minutes": 1, "seconds": 2, "frames": 12, "subframes": 3})
        validate_operation_payload("song.time-convert", "result", smpte_result)
        del song.get_beats_loop_length
        unavailable = mapper.invoke("song.time-convert", {"setRef": set_ref, "query": "beats-loop"})
        self.assertEqual(unavailable["available"], False)
        with self.assertRaisesRegex(ValueError, "query is invalid"): mapper.invoke("song.time-convert", {"setRef": set_ref, "query": "arbitrary-beats"})

    def test_song_time_convert_rejects_arbitrary_constant_tempo_math(self):
        song, mapper = self._mapper_with_song_state()
        set_ref = mapper.snapshot()["set"]["ref"]
        with self.assertRaisesRegex(ValueError, "arguments are invalid"):
            mapper.invoke("song.time-convert", {"setRef": set_ref, "beatTime": 4.0, "smpteSeconds": 10.0})


class TrackStructureExpansionTests(unittest.TestCase):
    def test_track_rows_expose_state_meters_and_view(self):
        song = FakeSong()
        track = song.tracks[0]
        track.is_visible = True; track.is_frozen = False; track.implicit_arm = False
        track.back_to_arranger = False; track.muted_via_solo = False
        track.input_meter_left = 0.5; track.input_meter_right = 0.4; track.input_meter_level = 0.45
        track.output_meter_left = 0.6; track.output_meter_right = 0.55; track.output_meter_level = 0.58
        track.performance_impact = 1
        track.view = type("TrackView", (), {"is_collapsed": False, "device_insert_mode": 1, "selected_device": track.devices[0]})()
        song.view = type("SongView", (), {"selected_track": track})()
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]
        self.assertEqual((row["isVisible"], row["isSelected"], row["isFrozen"], row["implicitArm"]), (True, True, False, False))
        self.assertEqual((row["outputMeterLeft"], row["outputMeterLevel"], row["performanceImpact"]), (0.6, 0.58, 1))
        self.assertEqual((row["view"]["isCollapsed"], row["view"]["deviceInsertMode"]), (False, 1))
        self.assertIsNotNone(row["view"]["selectedDeviceRef"])

    def test_track_color_rows_and_properties_set_with_rollback(self):
        song = FakeSong(); track = song.tracks[0]
        track.color_index = 5; track.color = 0xFF0000
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]
        self.assertEqual((row["colorIndex"], row["color"]), (5, 0xFF0000))
        self.assertTrue(mapper._operation_supported("track.set"))
        def fences():
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._track_properties_state(track)).encode()).hexdigest()}
        result = mapper.invoke("track.set", {**fences(), "colorIndex": 12})
        self.assertTrue(result["changed"]); validate_operation_payload("track.set", "result", result)
        self.assertEqual(track.color_index, 12)
        with self.assertRaisesRegex(ValueError, "colorIndex is invalid"): mapper.invoke("track.set", {**fences(), "colorIndex": 70})
        with self.assertRaisesRegex(ValueError, "colorIndex is invalid"): mapper.invoke("track.set", {**fences(), "colorIndex": True})
        with self.assertRaisesRegex(ValueError, "state changed since preview"): mapper.invoke("track.set", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": "0" * 64, "colorIndex": 3})
        with self.assertRaisesRegex(ValueError, "mutation has no fields"): mapper.invoke("track.set", fences())
        # A silently-refused write restores the exact prior value.
        class SometimesRefusingTrack(FakeTrack):
            @property
            def color_index(self): return self._color_index
            @color_index.setter
            def color_index(self, value):
                if getattr(self, "refuse", False): return
                self._color_index = value
        refusing = SometimesRefusingTrack(); refusing._color_index = 7; refusing.color = 0x00FF00
        song2 = FakeSong(); song2.tracks[0] = refusing
        mapper2 = LiveObjectMapper(song2)
        row2 = mapper2.snapshot()["tracks"][0]
        refusing.refuse = True
        with self.assertRaisesRegex(ValueError, "was not confirmed"):
            mapper2.invoke("track.set", {"ref": row2["ref"], "expectedObjectIdentity": row2["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper2._bounded_canonical(mapper2._track_properties_state(refusing)).encode()).hexdigest(), "colorIndex": 9})
        self.assertEqual(refusing.color_index, 7)
        # A shape without color keeps the field honestly null and refuses writes.
        plain_song = FakeSong(); plain = LiveObjectMapper(plain_song)
        plain_row = plain.snapshot()["tracks"][0]
        self.assertIsNone(plain_row["colorIndex"]); self.assertIsNone(plain_row["color"])
        self.assertFalse(plain._operation_supported("track.set"))
        with self.assertRaisesRegex(ValueError, "unavailable on this track"):
            plain.invoke("track.set", {"ref": plain_row["ref"], "expectedObjectIdentity": plain_row["objectIdentity"], "expectedStateRevision": hashlib.sha256(plain._bounded_canonical(plain._track_properties_state(plain_song.tracks[0])).encode()).hexdigest(), "colorIndex": 3})

    def test_return_track_create_and_delete_with_fencing(self):
        song = FakeSong()
        created_holder = []
        def create_return_track():
            track = FakeTrack(); track.name = "Return A"; created_holder.append(track); song.return_tracks.append(track); return track
        song.create_return_track = create_return_track
        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("track.create-return"))
        result = mapper.invoke("track.create-return", {"name": "Verb", "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual((result["name"], result["index"]), ("Verb", 0)); validate_operation_payload("track.create-return", "result", result)
        self.assertEqual(len(song.return_tracks), 1)
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("track.create-return", {"name": "X", "expectedStructureRevision": "0" * 64})
        self.assertTrue(mapper._operation_supported("track.delete-return"))
        deleted = mapper.invoke("track.delete-return", {"ref": result["ref"], "expectedObjectIdentity": result["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual(deleted, {"deleted": result["ref"]}); self.assertEqual(len(song.return_tracks), 0)

    def test_return_track_create_confirms_by_identity_diff_when_creator_returns_none(self):
        song = FakeSong()
        # list.append returns None, matching Live shapes whose creator has no
        # documented return value; confirmation must come from the identity-diff.
        song.create_return_track = lambda: song.return_tracks.append(FakeTrack())
        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        mapper = LiveObjectMapper(song)
        result = mapper.invoke("track.create-return", {"name": "Verb", "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual((result["name"], result["index"]), ("Verb", 0)); validate_operation_payload("track.create-return", "result", result)
        self.assertEqual(len(song.return_tracks), 1)

    def test_return_track_create_rolls_back_exactly_on_post_creation_failure(self):
        class NameRefusingTrack(FakeTrack):
            @property
            def name(self): return self._name
            @name.setter
            def name(self, value):
                if value != "Boom": self._name = value  # silently refuses "Boom": the write is not confirmed
        song = FakeSong()
        keep = FakeTrack(); keep.name = "Keep"; song.return_tracks = [keep]
        song.create_return_track = lambda: song.return_tracks.append(NameRefusingTrack())
        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        mapper = LiveObjectMapper(song)
        revision = mapper._structure_revision()
        with self.assertRaisesRegex(ValueError, "return-track name was not confirmed"):
            mapper.invoke("track.create-return", {"name": "Boom", "expectedStructureRevision": revision})
        self.assertEqual([track.name for track in song.return_tracks], ["Keep"])
        result = mapper.invoke("track.create-return", {"name": "Verb", "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual((result["name"], result["index"]), ("Verb", 1)); self.assertEqual(len(song.return_tracks), 2)

    def test_track_and_scene_duplication(self):
        song = FakeSong()
        def duplicate_track(index):
            import copy
            new = copy.copy(song.tracks[index]); song.tracks.insert(index + 1, new); return new
        def duplicate_scene(index):
            new = FakeScene(f"{song.scenes[index].name} copy"); song.scenes.insert(index + 1, new); return new
        song.duplicate_track = duplicate_track; song.duplicate_scene = duplicate_scene
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("track.duplicate")); self.assertTrue(mapper._operation_supported("scene.duplicate"))
        track_row = mapper.snapshot()["tracks"][0]
        result = mapper.invoke("track.duplicate", {"ref": track_row["ref"], "expectedObjectIdentity": track_row["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual(result["index"], 1); validate_operation_payload("track.duplicate", "result", result); self.assertEqual(len(song.tracks), 2)
        scene_row = mapper.snapshot()["scenes"][0]
        result = mapper.invoke("scene.duplicate", {"ref": scene_row["ref"], "expectedObjectIdentity": scene_row["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual(result["index"], 1); validate_operation_payload("scene.duplicate", "result", result); self.assertEqual(len(song.scenes), 2)

    def test_track_view_set_and_select_instrument(self):
        song = FakeSong()
        track = song.tracks[0]
        track.view = type("TrackView", (), {"is_collapsed": False, "device_insert_mode": 1, "selected_device": None})()
        selected = []
        track.view.select_instrument = lambda: selected.append(True)
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("track.view.set")); self.assertTrue(mapper._operation_supported("track.select-instrument"))
        row = mapper.snapshot()["tracks"][0]
        def fences(): return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": mapper._track_view_state_revision(track)}
        result = mapper.invoke("track.view.set", {**fences(), "collapsed": True, "deviceInsertMode": 2})
        self.assertTrue(result["changed"]); validate_operation_payload("track.view.set", "result", result)
        self.assertTrue(track.view.is_collapsed); self.assertEqual(track.view.device_insert_mode, 2)
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("track.view.set", {**fences(), "deviceInsertMode": 99})
        result = mapper.invoke("track.select-instrument", fences())
        self.assertEqual(result, {"done": True}); self.assertEqual(selected, [True])
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("track.view.set", fences())


class OwnershipAcrossTransactionsTests(unittest.TestCase):
    """Real-Live ownership rules: creations are cleaned up only with their transaction's token, later
    structure changes move positional references, and explicit deletions carry exact identity fences."""

    @staticmethod
    def song_with_returns():
        song = FakeSong()
        def create_return_track():
            track = FakeTrack(); track.name = f"Return {len(song.return_tracks)}"; song.return_tracks.append(track); return track
        song.create_return_track = create_return_track
        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        return song

    def test_a_return_made_earlier_no_longer_blocks_adding_tracks(self):
        # Returns' references follow every regular track's: a return Kumi made must not stop it from
        # ever adding a track again in that Live session.
        song = self.song_with_returns(); mapper = LiveObjectMapper(song, provenance="real-live")
        made = mapper.invoke("track.create-return", {"name": "Sweep Return", "expectedStructureRevision": mapper._structure_revision()}, "transaction-made-return")
        appended = mapper.invoke("track.create", {"name": "Bounce", "kind": "audio", "index": len(song.tracks), "expectedStructureRevision": mapper._structure_revision()}, "transaction-appended-track")
        self.assertEqual(([track.name for track in song.tracks], appended["index"]), (["Drums", "Bounce"], 1))
        again = mapper.invoke("track.create", {"name": "Bounce 2", "kind": "audio", "index": len(song.tracks), "expectedStructureRevision": mapper._structure_revision()}, "transaction-appended-again")
        self.assertEqual(again["name"], "Bounce 2")
        # The moved return's own undo is refused before anything changes, and the return stays.
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"):
            mapper.invoke("track.delete-return", {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-made-return", made["ownershipToken"])
        self.assertEqual([track.name for track in song.return_tracks], ["Sweep Return"])
        # The newest creation keeps its undo; undoing newest first moves the return back where it was
        # made, which gives it its ownership (and its undo) back.
        self.assertEqual(mapper.invoke("track.delete", {"ref": again["ref"], "expectedObjectIdentity": again["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-appended-again", again["ownershipToken"]), {"deleted": again["ref"]})
        self.assertEqual(mapper.invoke("track.delete", {"ref": appended["ref"], "expectedObjectIdentity": appended["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-appended-track", appended["ownershipToken"]), {"deleted": appended["ref"]})
        self.assertEqual(mapper.invoke("track.delete-return", {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-made-return", made["ownershipToken"]), {"deleted": made["ref"]})
        self.assertEqual((len(song.tracks), song.return_tracks), (1, []))

    def test_a_duplicated_track_is_undone_with_its_ownership_even_while_its_clip_plays(self):
        song = FakeSong(); clip = FakeClip(4.0); clip.name = "Loop"; clip.playing_position = 0.0; clip.is_playing = False; song.tracks[0].clip_slots[0].clip = clip
        def duplicate_track(index):
            original = song.tracks[index]; copy = FakeTrack(); copy.name = f"{original.name} copy"; copy.devices = []
            copied = FakeClip(4.0); copied.name = original.clip_slots[0].clip.name; copied.playing_position = 0.0; copied.is_playing = False; copy.clip_slots[0].clip = copied
            song.tracks.insert(index + 1, copy); return copy
        song.duplicate_track = duplicate_track
        mapper = LiveObjectMapper(song, provenance="real-live"); row = mapper.snapshot()["tracks"][0]
        duplicated = mapper.invoke("track.duplicate", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-duplicate")
        self.assertEqual(duplicated["createdFingerprint"], mapper._ownership_fingerprint(duplicated["ref"]))
        # The copy's clip plays when the undo comes: its playback is not an edit.
        copied = song.tracks[1].clip_slots[0].clip; copied.is_playing = True; copied.playing_position = 2.75
        deleted = mapper.invoke("track.delete", {"ref": duplicated["ref"], "expectedObjectIdentity": duplicated["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-duplicate", duplicated["ownershipToken"])
        self.assertEqual((deleted, [track.name for track in song.tracks]), ({"deleted": duplicated["ref"]}, ["Drums"]))

    def test_explicit_deletion_of_an_existing_device_and_return_uses_exact_identity_not_ownership(self):
        song = self.song_with_returns(); keep = FakeTrack(); keep.name = "A-Reverb"; song.return_tracks.append(keep)
        second = FakeDevice(); second.name = "Delay"; song.tracks[0].devices.append(second); song.tracks[0].delete_device = lambda index: song.tracks[0].devices.pop(index)
        mapper = LiveObjectMapper(song, provenance="real-live"); snapshot = mapper.snapshot(); track = snapshot["tracks"][0]; device = track["devices"][1]; siblings = [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in track["devices"]]
        delete_device = {"ref": device["ref"], "expectedObjectIdentity": device["objectIdentity"], "expectedOwnerRef": track["ref"], "expectedOwnerIdentity": track["objectIdentity"], "expectedSiblings": siblings, "expectedTrackRef": track["ref"], "expectedTrackIdentity": track["objectIdentity"]}
        # Without the explicit authority a deletion of something no transaction made is still refused.
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper.invoke("device.delete", delete_device, "transaction-foreign-delete")
        self.assertEqual(len(song.tracks[0].devices), 2)
        validate_operation_payload("device.delete", "request", {**delete_device, "explicitDeletion": True})
        # With it, the exact identity fences decide: a stale sibling fence refuses...
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("device.delete", {**delete_device, "expectedSiblings": siblings[:1], "explicitDeletion": True}, "transaction-stale-delete")
        # ...and the exact one deletes that one device.
        self.assertEqual(mapper.invoke("device.delete", {**delete_device, "explicitDeletion": True}, "transaction-device-delete"), {"deleted": device["ref"]})
        self.assertEqual([item.name for item in song.tracks[0].devices], ["Utility"])
        returned = next(row for row in mapper.snapshot()["tracks"] if row["name"] == "A-Reverb")
        delete_return = {"ref": returned["ref"], "expectedObjectIdentity": returned["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper.invoke("track.delete-return", delete_return, "transaction-foreign-return")
        validate_operation_payload("track.delete-return", "request", {**delete_return, "explicitDeletion": True})
        with self.assertRaisesRegex(ValueError, "identity changed"): mapper.invoke("track.delete-return", {**delete_return, "expectedObjectIdentity": "live:replacement", "explicitDeletion": True}, "transaction-stale-return")
        self.assertEqual(mapper.invoke("track.delete-return", {**delete_return, "explicitDeletion": True}, "transaction-return-delete"), {"deleted": returned["ref"]})
        self.assertEqual(song.return_tracks, [])
        # The authority is for the deletions a producer confirms (devices, returns, tracks, scenes, clips,
        # Arrangement clips, locators), only when named: anything else still needs its creation's token.
        clip_ref = f"{mapper.refs.epoch}:clip:0:0"
        self.assertTrue(remote_module._explicit_deletion("clip.delete", {"ref": clip_ref, "explicitDeletion": True}))
        self.assertFalse(remote_module._explicit_deletion("clip.delete", {"ref": clip_ref, "explicitDeletion": False}))
        self.assertFalse(remote_module._explicit_deletion("note.delete", {"ref": clip_ref, "explicitDeletion": True}))

    def test_an_explicitly_deleted_creation_loses_its_ownership(self):
        song = self.song_with_returns(); mapper = LiveObjectMapper(song, provenance="real-live")
        made = mapper.invoke("track.create-return", {"name": "Made", "expectedStructureRevision": mapper._structure_revision()}, "transaction-made")
        later = mapper.invoke("track.create-return", {"name": "Later", "expectedStructureRevision": mapper._structure_revision()}, "transaction-later")
        mapper.invoke("track.delete-return", {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"], "expectedStructureRevision": mapper._structure_revision(), "explicitDeletion": True}, "transaction-explicit")
        self.assertEqual([track.name for track in song.return_tracks], ["Later"])
        # The deleted return's own undo and the moved later return's undo are refused before anything changes.
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper._require_cleanup_ownership("track.delete-return", {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"]}, "transaction-made", made["ownershipToken"])
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper._require_cleanup_ownership("track.delete-return", {"ref": later["ref"], "expectedObjectIdentity": later["objectIdentity"]}, "transaction-later", later["ownershipToken"])


class SelectionViewExpansionTests(unittest.TestCase):
    def test_selection_set_assigns_song_view_selections(self):
        song = FakeSong()
        track = song.tracks[0]; scene = song.scenes[0]; slot = track.clip_slots[0]; device = track.devices[0]; parameter = device.parameters[0]
        song.view = type("SongView", (), {"selected_track": None, "selected_scene": None, "highlighted_clip_slot": None, "detail_clip": None, "selected_device": None, "selected_parameter": None, "selected_chain": None})()
        clip = FakeClip(4.0); slot.clip = clip
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("selection.set"))
        snapshot = mapper.snapshot()
        track_ref = snapshot["tracks"][0]["ref"]; scene_ref = snapshot["scenes"][0]["ref"]; slot_ref = snapshot["tracks"][0]["clipSlots"][0]["ref"]; clip_ref = snapshot["tracks"][0]["clips"][0]["ref"]
        device_ref = snapshot["tracks"][0]["devices"][0]["ref"]; parameter_ref = snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]
        args = {"trackRef": track_ref, "sceneRef": scene_ref, "slotRef": slot_ref, "detailClipRef": clip_ref, "deviceRef": device_ref, "parameterRef": parameter_ref, "expectedStateRevision": mapper._selection_revision()}
        result = mapper.invoke("selection.set", args)
        self.assertTrue(result["changed"]); validate_operation_payload("selection.set", "result", result)
        self.assertIs(song.view.selected_track, track); self.assertIs(song.view.selected_scene, scene)
        self.assertIs(song.view.highlighted_clip_slot, slot); self.assertIs(song.view.detail_clip, clip)
        self.assertIs(song.view.selected_parameter, parameter)
        # Live keeps the selected device on the selected track's view: that's what the snapshot and the focus feed name.
        track.view = type("TrackView", (), {"selected_device": device})()
        self.assertEqual(mapper.snapshot()["selection"]["deviceRef"], device_ref, "read from the selected track")
        self.assertEqual(mapper.discover("selection")["items"][0]["selectedDeviceRef"], device_ref, "the focus feed names the selected device")
        # Live's device type goes on each device row.
        device.type = 2; self.assertEqual(mapper.snapshot()["tracks"][0]["devices"][0]["deviceType"], "audio_effect")
        device.type = 1; self.assertEqual(mapper.snapshot()["tracks"][0]["devices"][0]["deviceType"], "instrument")
        del device.type; self.assertIsNone(mapper.snapshot()["tracks"][0]["devices"][0]["deviceType"])
        cleared = mapper.invoke("selection.set", {"detailClipRef": None, "expectedStateRevision": mapper._selection_revision()})
        self.assertTrue(cleared["changed"]); self.assertIsNone(song.view.detail_clip)
        stale = dict(args, expectedStateRevision="0" * 64)
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("selection.set", stale)

    def test_the_snapshot_names_the_selection_the_host_fences_selection_changes_on(self):
        # The host previews from the snapshot: its selectionRevision hashes snapshot.selection. The bridge
        # must check that same state, named by the snapshot's own references.
        song = FakeSong(); second = FakeTrack(); second.name = "Reese Bass"; song.tracks.append(second); scene = song.scenes[0]
        volume = FakeParameter(); song.tracks[0].mixer_device = type("Mixer", (), {"volume": volume, "panning": FakeParameter(), "sends": []})()
        song.view = type("SongView", (), {"selected_track": song.tracks[0], "selected_scene": scene, "highlighted_clip_slot": song.tracks[0].clip_slots[0], "detail_clip": None, "selected_parameter": volume, "selected_chain": None})()
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot()
        validate_operation_payload("snapshot", "result", snapshot)
        self.assertEqual(snapshot["selection"], {"trackRef": snapshot["tracks"][0]["ref"], "sceneRef": snapshot["scenes"][0]["ref"], "slotRef": snapshot["tracks"][0]["clipSlots"][0]["ref"], "detailClipRef": None, "deviceRef": None, "parameterRef": snapshot["tracks"][0]["mixer"]["volumeRef"], "chainRef": None})
        host_revision = hashlib.sha256(mapper._bounded_canonical({key: snapshot["selection"].get(key) for key in ("trackRef", "sceneRef", "slotRef", "detailClipRef", "deviceRef", "parameterRef", "chainRef")}).encode()).hexdigest()
        request = {"trackRef": snapshot["tracks"][1]["ref"], "expectedStateRevision": host_revision}
        validate_operation_payload("selection.set", "request", request)
        self.assertTrue(mapper.invoke("selection.set", request)["changed"])
        self.assertIs(song.view.selected_track, second); self.assertEqual(mapper.snapshot()["selection"]["trackRef"], snapshot["tracks"][1]["ref"])

    def test_selection_set_rejects_draw_mode_and_other_non_selection_fields(self):
        song = FakeSong()
        song.view = type("SongView", (), {"selected_track": None, "selected_scene": None, "highlighted_clip_slot": None, "detail_clip": None, "selected_device": None, "selected_parameter": None, "selected_chain": None})()
        mapper = LiveObjectMapper(song)
        track_ref = mapper.snapshot()["tracks"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "selection fields are invalid"):
            mapper.invoke("selection.set", {"trackRef": track_ref, "drawMode": False, "expectedStateRevision": mapper._selection_revision()})
        with self.assertRaisesRegex(ValueError, "selection fields are invalid"):
            mapper.invoke("selection.set", {"trackRef": track_ref, "unexpected": 1, "expectedStateRevision": mapper._selection_revision()})
        self.assertIsNone(song.view.selected_track)

    def test_song_view_draw_mode_clip_view_and_device_view(self):
        song = FakeSong()
        song.view = type("SongView", (), {"draw_mode": False})()
        clip = FakeClip(4.0)
        clip.view = type("ClipView", (), {"grid_quantization": 1, "grid_is_triplet": False})()
        clip.view.show_loop = lambda: setattr(clip.view, "_loop_shown", True)
        clip.view.show_envelope = lambda: setattr(clip.view, "_envelope_shown", True)
        clip.view.hide_envelope = lambda: setattr(clip.view, "_envelope_shown", False)
        song.tracks[0].clip_slots[0].clip = clip
        device = song.tracks[0].devices[0]
        device.view = type("DeviceView", (), {"is_collapsed": False})()
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("song.view.set"))
        draw_revision = hashlib.sha256(mapper._bounded_canonical({"drawMode": False}).encode()).hexdigest()
        result = mapper.invoke("song.view.set", {"drawMode": True, "expectedStateRevision": draw_revision})
        self.assertTrue(result["changed"]); validate_operation_payload("song.view.set", "result", result); self.assertTrue(song.view.draw_mode)
        self.assertTrue(mapper._operation_supported("clip.view.set"))
        row = mapper.snapshot()["tracks"][0]["clips"][0]
        def clip_fences():
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._clip_view_state(clip)).encode()).hexdigest()}
        result = mapper.invoke("clip.view.set", {**clip_fences(), "gridQuantization": 4, "gridIsTriplet": True, "showEnvelope": True, "showLoop": True})
        self.assertTrue(result["changed"]); validate_operation_payload("clip.view.set", "result", result)
        self.assertEqual((clip.view.grid_quantization, clip.view.grid_is_triplet), (4, True))
        self.assertTrue(getattr(clip.view, "_envelope_shown", False))
        self.assertTrue(getattr(clip.view, "_loop_shown", False))
        self.assertEqual(row["clipView"], {"gridQuantization": 1, "gridIsTriplet": False})
        self.assertTrue(mapper._operation_supported("device.view.set"))
        device_row = mapper.snapshot()["tracks"][0]["devices"][0]
        collapsed_revision = hashlib.sha256(mapper._bounded_canonical({"collapsed": False}).encode()).hexdigest()
        result = mapper.invoke("device.view.set", {"ref": device_row["ref"], "collapsed": True, "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": collapsed_revision})
        self.assertTrue(result["changed"]); validate_operation_payload("device.view.set", "result", result)
        self.assertTrue(device.view.is_collapsed)

    def test_clip_view_set_rejects_non_clip_refs_with_clean_authority_error(self):
        song = FakeSong()
        clip = FakeClip(4.0)
        clip.view = type("ClipView", (), {"grid_quantization": 1, "grid_is_triplet": False})()
        song.tracks[0].clip_slots[0].clip = clip
        track = song.tracks[0]
        track.view = type("TrackView", (), {"grid_quantization": 1, "grid_is_triplet": False})()
        mapper = LiveObjectMapper(song)
        track_row = mapper.snapshot()["tracks"][0]
        with self.assertRaisesRegex(ValueError, "clip view authority is invalid"):
            mapper.invoke("clip.view.set", {"ref": track_row["ref"], "gridQuantization": 4, "expectedObjectIdentity": track_row["objectIdentity"], "expectedStateRevision": "0" * 64})
        self.assertEqual((track.view.grid_quantization, track.view.grid_is_triplet), (1, False))

    def test_application_dialog_read_and_guarded_press(self):
        class FakeApp:
            def __init__(self):
                self.current_dialog_button_count = 2; self.current_dialog_message = "Save changes before closing?"; self.open_dialog_count = 1; self.pressed = []
            def press_current_dialog_button(self, button):
                if not isinstance(button, int) or button >= self.current_dialog_button_count: raise RuntimeError("no such button")
                self.pressed.append(button); self.open_dialog_count = 0
        song = FakeSong(); app = FakeApp()
        mapper = LiveObjectMapper(song); mapper._application = lambda: app
        self.assertTrue(mapper._operation_supported("application.dialog"))
        result = mapper.invoke("application.dialog", {"action": "read"})
        self.assertEqual(result, {"buttonCount": 2, "message": "Save changes before closing?", "openDialogCount": 1, "done": True}); validate_operation_payload("application.dialog", "result", result)
        with self.assertRaisesRegex(ValueError, "changed since preview"):
            mapper.invoke("application.dialog", {"action": "press", "button": 1, "expectedMessage": "Discard everything?", "expectedButtonCount": 2, "expectedOpenDialogCount": 1})
        with self.assertRaisesRegex(ValueError, "not present"):
            mapper.invoke("application.dialog", {"action": "press", "button": 5, "expectedMessage": "Save changes before closing?", "expectedButtonCount": 2, "expectedOpenDialogCount": 1})
        result = mapper.invoke("application.dialog", {"action": "press", "button": 1, "expectedMessage": "Save changes before closing?", "expectedButtonCount": 2, "expectedOpenDialogCount": 1})
        self.assertEqual(app.pressed, [1]); self.assertEqual(result["openDialogCount"], 0); validate_operation_payload("application.dialog", "result", result)
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("application.dialog", {"action": "press", "button": 1, "expectedState": 1})

    def test_view_control_browser_toggle_and_hide_focus(self):
        class FakeAppView:
            def __init__(self): self.visible = "Session"; self.toggles = 0; self.hidden = []; self.focused = []
            def show_view(self, name): self.visible = name
            def is_view_visible(self, name): return self.visible == name
            def zoom_view(self, *args): pass
            def scroll_view(self, *args): pass
            def toggle_browse(self): self.toggles += 1
            def hide_view(self, name): self.hidden.append(name)
            def focus_view(self, name): self.focused.append(name)
        class FakeApplication: pass
        song = FakeSong(); song.view = type("SongView", (), {"follow_song": False})()
        application = FakeApplication(); application.view = FakeAppView()
        mapper = LiveObjectMapper(song); mapper._application = lambda: application
        result = mapper.invoke("view.control", {"action": "browser-toggle"})
        self.assertEqual(result, {"action": "browser-toggle", "done": True}); self.assertEqual(application.view.toggles, 1)
        mapper.invoke("view.control", {"action": "hide-view", "view": "Browser"})
        mapper.invoke("view.control", {"action": "focus-view", "view": "Arranger"})
        self.assertEqual(application.view.hidden, ["Browser"]); self.assertEqual(application.view.focused, ["Arranger"])
        with self.assertRaisesRegex(ValueError, "view name is required"): mapper.invoke("view.control", {"action": "hide-view"})


class PerformanceDiagnosticsTests(unittest.TestCase):
    def test_performance_read_exposes_usage_meters_and_latency(self):
        class FakeApp:
            average_process_usage = 0.42
            peak_process_usage = 0.87
        song = FakeSong()
        song.tracks[0].devices[0].latency_in_samples = 256
        song.tracks[0].devices[0].latency_in_ms = 5.8
        track = song.tracks[0]
        track.performance_impact = 1
        track.input_meter_left = 0.5; track.input_meter_right = 0.4; track.input_meter_level = 0.45
        track.output_meter_left = 0.6; track.output_meter_right = 0.55; track.output_meter_level = 0.58
        mapper = LiveObjectMapper(song); mapper._application = lambda: FakeApp()
        set_ref = mapper.snapshot()["set"]["ref"]
        result = mapper.invoke("performance.read", {"setRef": set_ref})
        self.assertEqual((result["averageProcessUsage"], result["peakProcessUsage"]), (0.42, 0.87))
        self.assertIsInstance(result["sampledAt"], int); self.assertEqual(len(result["revision"]), 64)
        row = result["tracks"][0]
        self.assertEqual((row["performanceImpact"], row["outputMeterLevel"]), (1, 0.58))
        self.assertEqual((row["devices"][0]["latencySamples"], row["devices"][0]["latencyMs"]), (256, 5.8))
        validate_operation_payload("performance.read", "result", result)
        device_row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual((device_row["latencySamples"], device_row["latencyMs"]), (256, 5.8))


class FakeMixerDevice:
    def __init__(self):
        self.volume = FakeParameter(); self.panning = FakeParameter(); self.cue_volume = FakeParameter()
        self.sends = [FakeParameter(), FakeParameter()]
        self.track_activator = FakeParameter(); self.track_activator.value = 1.0; self.track_activator.quantization = 1.0
        self.crossfader = FakeParameter(); self.crossfader.value = 0.0; self.crossfader.min = -1.0; self.crossfader.max = 1.0
        self.crossfade_assign = 1
        self.panning_mode = 0
        self.left_split_stereo = FakeParameter(); self.left_split_stereo.value = 0.0; self.left_split_stereo.min = -1.0; self.left_split_stereo.max = 1.0
        self.right_split_stereo = FakeParameter(); self.right_split_stereo.value = 0.0; self.right_split_stereo.min = -1.0; self.right_split_stereo.max = 1.0
        self.chain_activator = FakeParameter(); self.chain_activator.value = 1.0; self.chain_activator.quantization = 1.0
        self.song_tempo = FakeParameter(); self.song_tempo.value = 120.0; self.song_tempo.min = 20.0; self.song_tempo.max = 999.0


class MixerRoutingExpansionTests(unittest.TestCase):
    def _mapper_with_mixer(self):
        song = FakeSong()
        song.tracks[0].mixer_device = FakeMixerDevice()
        return song, LiveObjectMapper(song)

    def test_mixer_rows_carry_lives_own_text_for_values(self):
        song, mapper = self._mapper_with_mixer()
        mixer = song.tracks[0].mixer_device
        mixer.volume.value = 0.85; mixer.volume.str_for_value = lambda value: "0.0 dB" if value == 0.85 else f"{value:.2f}"
        mixer.panning.value = -0.5; mixer.panning.str_for_value = lambda value: "25L"
        mixer.sends[0].str_for_value = lambda value: "-inf dB"
        mixer.sends[1].str_for_value = lambda value: 1 / 0
        row = mapper.snapshot()["tracks"][0]["mixer"]
        self.assertEqual((row["volumeDisplay"], row["panDisplay"]), ("0.0 dB", "25L"))
        self.assertIsNone(row["cueVolumeDisplay"], "a parameter without Live's text has none")
        self.assertEqual(row["sendDisplays"], ["-inf dB", None], "a failing formatter is skipped, not fatal")

    def test_mixer_extended_fields_and_set(self):
        song, mapper = self._mapper_with_mixer()
        row = mapper.snapshot()["tracks"][0]["mixer"]
        self.assertIsNotNone(row["trackActivatorRef"]); self.assertIsNotNone(row["crossfaderRef"])
        self.assertEqual((row["crossfadeAssign"], row["panningMode"]), (1, 0))
        self.assertIsNotNone(row["panningLeftRef"]); self.assertIsNotNone(row["panningRightRef"])
        track_row = mapper.snapshot()["tracks"][0]
        mixer = song.tracks[0].mixer_device
        def fences():
            return {"ref": track_row["ref"], "expectedObjectIdentity": track_row["objectIdentity"], "expectedMixerIdentity": mapper._capture_object_identity(mixer),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"crossfadeAssign": mixer.crossfade_assign, "panningMode": mixer.panning_mode}).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("mixer.extended.set"))
        result = mapper.invoke("mixer.extended.set", {**fences(), "trackActivator": False, "crossfader": -0.5, "crossfadeAssign": 0, "panningMode": 1, "panningLeft": -0.25, "panningRight": 0.25})
        self.assertTrue(result["changed"]); validate_operation_payload("mixer.extended.set", "result", result)
        self.assertEqual(mixer.track_activator.value, 0.0); self.assertEqual(mixer.crossfader.value, -0.5)
        self.assertEqual((mixer.crossfade_assign, mixer.panning_mode), (0, 1))
        self.assertEqual((mixer.left_split_stereo.value, mixer.right_split_stereo.value), (-0.25, 0.25))
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("mixer.extended.set", {**fences(), "crossfader": 2.0})
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("mixer.extended.set", {**fences(), "crossfadeAssign": 3})

    def test_the_snapshot_mixer_row_carries_the_extended_mixer_authority_the_host_previews(self):
        # The host previews from the snapshot alone: the track mixer row must name the mixer itself and
        # the values it edits (chain mixers already do), or the preview refuses as not authoritative.
        song, mapper = self._mapper_with_mixer()
        track_row = mapper.snapshot()["tracks"][0]; row = track_row["mixer"]
        self.assertEqual(row["mixerIdentity"], mapper._capture_object_identity(song.tracks[0].mixer_device))
        self.assertEqual((row["trackActivator"], row["crossfader"], row["panningLeft"], row["panningRight"]), (True, 0.0, 0.0, 0.0))
        state = {"crossfadeAssign": row["crossfadeAssign"], "panningMode": row["panningMode"]}
        request = {"ref": track_row["ref"], "trackActivator": False, "expectedObjectIdentity": track_row["objectIdentity"], "expectedMixerIdentity": row["mixerIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        validate_operation_payload("mixer.extended.set", "request", request)
        self.assertTrue(mapper.invoke("mixer.extended.set", request)["changed"])
        self.assertIs(mapper.snapshot()["tracks"][0]["mixer"]["trackActivator"], False)

    def test_mixer_extended_resolves_return_and_main_tracks(self):
        song = FakeSong()
        return_track = type("Track", (), {"name": "Return A", "mixer_device": FakeMixerDevice(), "devices": []})()
        main_track = type("Track", (), {"name": "Main", "mixer_device": FakeMixerDevice(), "devices": []})()
        song.return_tracks = [return_track]; song.master_track = main_track
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot()
        rows = {row["name"]: row for row in snapshot["tracks"]}
        for name, track in (("Return A", return_track), ("Main", main_track)):
            row = rows[name]
            mixer = track.mixer_device
            fences = {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedMixerIdentity": mapper._capture_object_identity(mixer),
                      "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"crossfadeAssign": mixer.crossfade_assign, "panningMode": mixer.panning_mode}).encode()).hexdigest()}
            result = mapper.invoke("mixer.extended.set", {**fences, "crossfader": -0.5})
            self.assertTrue(result["changed"]); self.assertEqual(mixer.crossfader.value, -0.5)

    def test_chain_mixer_fields_and_set(self):
        song = FakeSong()
        chain = type("Chain", (), {"name": "Chain 1", "devices": [], "mute": False, "solo": False, "mixer_device": FakeMixerDevice()})()
        rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True; rack.chains = [chain]
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        chain_row = mapper.snapshot()["tracks"][0]["devices"][0]["chains"][0]
        self.assertIn("mixer", chain_row); self.assertIsNotNone(chain_row["mixer"]["volumeRef"])
        mixer = chain.mixer_device
        def fences():
            return {"ref": chain_row["ref"], "expectedObjectIdentity": chain_row["objectIdentity"], "expectedMixerIdentity": mapper._capture_object_identity(mixer),
                    "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"sends": [send.value for send in mixer.sends]}).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("chain-mixer.set"))
        # What the host fences with comes from the snapshot, as on real Live.
        self.assertEqual(chain_row["mixer"]["mixerIdentity"], mapper._capture_object_identity(mixer)); self.assertIs(chain_row["mixer"]["chainActivator"], True)
        result = mapper.invoke("chain-mixer.set", {**fences(), "volume": 0.5, "pan": -0.5, "sends": [0.25, 0.75], "chainActivator": False})
        self.assertTrue(result["changed"]); validate_operation_payload("chain-mixer.set", "result", result)
        self.assertIs(mapper.snapshot()["tracks"][0]["devices"][0]["chains"][0]["mixer"]["chainActivator"], False)
        self.assertEqual((mixer.volume.value, mixer.panning.value), (0.5, -0.5))
        self.assertEqual([send.value for send in mixer.sends], [0.25, 0.75]); self.assertEqual(mixer.chain_activator.value, 0.0)
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("chain-mixer.set", {**fences(), "sends": [0.5, 0.5, 0.5]})

    def test_max_device_ios_read_the_same_every_time(self):
        # Live hands out a fresh DeviceIO wrapper on every read: a row that showed its address could never be
        # confirmed after a load (a Max audio effect's load stayed "uncertain").
        song = FakeSong()
        device = song.tracks[0].devices[0]
        device.class_name = "MaxDevice"
        class FreshIo:
            def __init__(self, routing): self.routing_type = {"name": routing}
        type(device).audio_inputs = property(lambda self: [FreshIo("No Input")])
        type(device).audio_outputs = property(lambda self: [FreshIo("Main"), FreshIo("Main")])
        try:
            mapper = LiveObjectMapper(song)
            first = mapper.snapshot()["tracks"][0]["devices"][0]["maxDevice"]
            second = LiveObjectMapper(song).snapshot()["tracks"][0]["devices"][0]["maxDevice"]
            self.assertEqual(first, second)
            self.assertEqual(first["audioIns"], ["No Input"]); self.assertEqual(first["audioOuts"], ["Main", "Main"])
            self.assertNotIn(" at 0x", json.dumps(first))
        finally:
            del type(device).audio_inputs; del type(device).audio_outputs

    def test_device_io_and_compressor_sidechain_shape_gated(self):
        song = FakeSong()
        device = song.tracks[0].devices[0]
        class FakeChoice:
            def __init__(self, name): self.name = name
        class FakeIo:
            def __init__(self):
                self.available_routing_types = [{"name": "Ext. In"}, {"name": "Main"}]
                self.available_routing_channels = [{"name": "1"}, {"name": "1/2"}]
                self.routing_type = self.available_routing_types[0]
                self.routing_channel = self.available_routing_channels[0]
                self.default_external_routing_channel_is_none = True
        device.audio_inputs = [FakeIo()]
        device.available_input_routing_types = [{"name": "None"}, {"name": "Ext. In"}]
        device.input_routing_type = device.available_input_routing_types[0]
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("device-io.set")); self.assertTrue(mapper._operation_supported("compressor.sidechain.set"))
        device_row = mapper.snapshot()["tracks"][0]["devices"][0]
        io_state = mapper._device_io_fields(device)
        io_revision = hashlib.sha256(mapper._bounded_canonical({"routingType": io_state["routingType"], "routingChannel": io_state["routingChannel"]}).encode()).hexdigest()
        result = mapper.invoke("device-io.set", {"ref": device_row["ref"], "routingType": "Main", "routingChannel": "1/2", "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": io_revision})
        self.assertTrue(result["changed"]); validate_operation_payload("device-io.set", "result", result)
        self.assertEqual(device.audio_inputs[0].routing_type["name"], "Main"); self.assertEqual(device.audio_inputs[0].routing_channel["name"], "1/2")
        fresh_revision = hashlib.sha256(mapper._bounded_canonical({"routingType": "Main", "routingChannel": "1/2"}).encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, "not an available choice"): mapper.invoke("device-io.set", {"ref": device_row["ref"], "routingType": "Bogus", "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": fresh_revision})
        sc_revision = hashlib.sha256(mapper._bounded_canonical({"routingType": "None"}).encode()).hexdigest()
        result = mapper.invoke("compressor.sidechain.set", {"ref": device_row["ref"], "routingType": "Ext. In", "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": sc_revision})
        self.assertTrue(result["changed"]); validate_operation_payload("compressor.sidechain.set", "result", result)
        self.assertEqual(device.input_routing_type["name"], "Ext. In")
        self.assertEqual(device_row["sidechainRoutingType"], "None")
        self.assertEqual(device_row["deviceIo"]["routingType"], "Ext. In")


class DeviceParameterExpansionTests(unittest.TestCase):
    def test_parameter_rows_expose_metadata_and_device_bank_comparison(self):
        song = FakeSong()
        parameter = song.tracks[0].devices[0].parameters[0]
        parameter.default_value = 0.75; parameter.original_name = "Gain (dB)"; parameter.state = 1
        parameter.value_items = ["Off", "On"]
        device = song.tracks[0].devices[0]
        device.parameter_bank_count = lambda: 3; device.can_compare_ab = True; device.is_using_compare_preset_b = False
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        self.assertEqual((row["defaultValue"], row["originalName"], row["state"]), (0.75, "Gain (dB)", 1))
        self.assertEqual(row["valueItems"], ["Off", "On"])
        device_row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(device_row["parameterBank"], 3)
        self.assertEqual(device_row["comparison"], {"capability": True, "activeSide": 0})

    def test_device_bank_set_stores_chosen_bank(self):
        song = FakeSong()
        device = song.tracks[0].devices[0]
        device.parameter_bank_count = lambda: 4
        stored = []
        device.store_chosen_bank = lambda script_index, bank_index: stored.append((script_index, bank_index))
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        def fences(): return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"bankCount": 4}).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("device.bank.set"))
        result = mapper.invoke("device.bank.set", {**fences(), "bank": 2, "scriptIndex": 0})
        self.assertTrue(result["changed"]); validate_operation_payload("device.bank.set", "result", result)
        self.assertEqual(stored, [(0, 2)])
        with self.assertRaisesRegex(ValueError, "exceeds"): mapper.invoke("device.bank.set", {**fences(), "bank": 4})
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("device.bank.set", {**fences(), "bank": -1})

    def test_parameter_re_enable_automation_and_comparison_save(self):
        song = FakeSong()
        parameter = song.tracks[0].devices[0].parameters[0]
        called = []
        parameter.re_enable_automation = lambda: called.append(True)
        device = song.tracks[0].devices[0]
        stored = []
        device.can_compare_ab = True; device.is_using_compare_preset_b = False
        device.save_preset_to_compare_ab_slot = lambda: stored.append(True)
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("parameter.re-enable-automation")); self.assertTrue(mapper._operation_supported("device.comparison.save-to-slot"))
        parameter_row = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        result = mapper.invoke("parameter.re-enable-automation", {"ref": parameter_row["ref"], "expectedObjectIdentity": parameter_row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"automationState": "none"}).encode()).hexdigest()})
        self.assertEqual(result, {"done": True}); validate_operation_payload("parameter.re-enable-automation", "result", result); self.assertEqual(called, [True])
        device_row = mapper.snapshot()["tracks"][0]["devices"][0]
        comparison_revision = hashlib.sha256(mapper._bounded_canonical({"canCompareAb": True, "isUsingComparePresetB": False}).encode()).hexdigest()
        result = mapper.invoke("device.comparison.save-to-slot", {"ref": device_row["ref"], "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": comparison_revision})
        self.assertEqual(result, {"done": True}); self.assertEqual(stored, [True])
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("device.comparison.save-to-slot", {"ref": device_row["ref"], "slot": 1, "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": comparison_revision})
        device.can_compare_ab = False
        with self.assertRaisesRegex(ValueError, "unavailable"):
            mapper.invoke("device.comparison.save-to-slot", {"ref": device_row["ref"], "expectedObjectIdentity": device_row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"canCompareAb": False, "isUsingComparePresetB": False}).encode()).hexdigest()})

    def test_parameter_re_enable_automation_accepts_rack_macro_refs(self):
        song = FakeSong()
        rack = FakeRackDevice()
        macro = rack.macros[0]
        called = []
        macro.re_enable_automation = lambda: called.append(True)
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        macro_row = mapper.snapshot()["tracks"][0]["devices"][0]["macros"][0]
        self.assertTrue(macro_row["ref"].endswith(":macro:0"))
        result = mapper.invoke("parameter.re-enable-automation", {"ref": macro_row["ref"], "expectedObjectIdentity": macro_row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"automationState": "none"}).encode()).hexdigest()})
        self.assertEqual(result, {"done": True}); validate_operation_payload("parameter.re-enable-automation", "result", result); self.assertEqual(called, [True])

    def test_cross_track_and_chain_device_move(self):
        song = FakeSong()
        source = FakeDevice(); source.name = "Mover"
        song.tracks[0].devices = [source]
        target_track = FakeTrack(); target_track.name = "Target"; target_track.devices = []
        song.tracks.append(target_track)
        def move_device(device, target, position):
            for owner in song.tracks:
                if device in owner.devices: owner.devices.remove(device)
            target.devices.insert(position, device)
        song.move_device = move_device
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("device.move"))
        snapshot = mapper.snapshot()
        source_row = snapshot["tracks"][0]["devices"][0]; target_row = snapshot["tracks"][1]
        args = {"ref": source_row["ref"], "index": 0, "targetTrackRef": target_row["ref"], "expectedTargetIdentity": target_row["objectIdentity"],
                "expectedObjectIdentity": source_row["objectIdentity"], "expectedOwnerRef": snapshot["tracks"][0]["ref"], "expectedOwnerIdentity": snapshot["tracks"][0]["objectIdentity"],
                "expectedSiblings": [{"ref": source_row["ref"], "objectIdentity": source_row["objectIdentity"]}], "expectedTrackRef": snapshot["tracks"][0]["ref"], "expectedTrackIdentity": snapshot["tracks"][0]["objectIdentity"]}
        result = mapper.invoke("device.move", args)
        self.assertEqual(result["index"], 0); self.assertEqual(len(song.tracks[0].devices), 0); self.assertEqual(song.tracks[1].devices, [source])

    def test_chain_device_insert_shape_gated(self):
        song = FakeSong()
        chain = type("Chain", (), {"name": "Chain 1", "devices": [], "mute": False, "solo": False})()
        def insert_device(name, index=-1):
            device = FakeDevice(); device.name = name; chain.devices.append(device); return device
        chain.insert_device = insert_device
        rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True; rack.chains = [chain]
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("device.insert"))
        track_row = mapper.snapshot()["tracks"][0]; chain_row = track_row["devices"][0]["chains"][0]
        result = mapper.invoke("device.insert", {"trackRef": track_row["ref"], "chainRef": chain_row["ref"], "deviceName": "Inserted", "expectedTrackIdentity": track_row["objectIdentity"], "expectedSiblings": []})
        self.assertEqual(len(chain.devices), 1); self.assertEqual(chain.devices[0].name, "Inserted")


class FakeRackView:
    def __init__(self):
        self.selected_chain = None
        self.selected_drum_pad = None
        self.drum_pads_scroll_position = 0
        self.is_showing_chain_devices = True


class FakeMacro:
    def __init__(self, name):
        self.name = name
        self.value = 0.0


class FakeRackDevice:
    def __init__(self):
        self.name = "Rack"
        self.class_name = "RackDevice"
        self.can_have_chains = True
        self.enabled = True
        self.parameters = []
        self.chains = []
        self.return_chains = []
        self.macros = [FakeMacro("Macro 1")]
        self.macros_mapped = [True]
        self.visible_macro_count = 8
        self.variation_count = 1
        self.selected_variation_index = 0
        self.view = FakeRackView()

    def add_macro(self): self.visible_macro_count += 1
    def remove_macro(self):
        if self.visible_macro_count <= 1: raise RuntimeError("cannot remove the last macro")
        self.visible_macro_count -= 1
    def randomize_macros(self): pass
    def insert_chain(self, index=-1):
        chain = type("Chain", (), {"name": "New Chain", "devices": [], "mute": False, "solo": False})()
        self.chains.append(chain); return chain
    def copy_pad(self, source, target): pass
    def store_variation(self): self.variation_count += 1
    def recall_selected_variation(self): pass
    def delete_selected_variation(self): self.variation_count -= 1


def host_rack_state_revision(device_row):
    """The host's rack state revision (host/racks.rs `rack_state_revision`), from a snapshot rack row: what rack edits are fenced on."""
    state = {"visibleMacroCount": device_row.get("visibleMacroCount"), "selectedVariationIndex": device_row.get("selectedVariationIndex"), "variationCount": device_row.get("variationCount"),
             "macros": [macro["objectIdentity"] for macro in device_row.get("macros") or []], "chains": [chain["objectIdentity"] for chain in device_row.get("chains") or []], "drumPads": [pad["objectIdentity"] for pad in device_row.get("drumPads") or []]}
    return hashlib.sha256(AuthenticatedRemoteScript._bounded_canonical(state).encode()).hexdigest()


def host_capture_authority_revision(snapshot):
    """The host's capture authority revision (host/session_capture.rs `capture_authority_revision`), over the snapshot as the host receives it."""
    snapshot = json.loads(json.dumps(snapshot))
    authority = {"tracks": [{"ref": track["ref"], "objectIdentity": track["objectIdentity"], "clips": [{"ref": clip["ref"], "objectIdentity": clip["objectIdentity"], "notesRevision": clip["notesRevision"]} for clip in track["clips"]]} for track in snapshot["tracks"]],
                 "scenes": [{"ref": scene["ref"], "objectIdentity": scene["objectIdentity"], "index": scene["index"]} for scene in snapshot["scenes"]], "playbackRevision": snapshot["playback"]["revision"]}
    return hashlib.sha256(AuthenticatedRemoteScript._bounded_canonical(authority).encode()).hexdigest()


class CaptureAuthorityAgreementTests(unittest.TestCase):
    def test_scene_capture_is_fenced_on_the_state_the_host_previews(self):
        song = FakeSong(); clip = FakeClip(4.0); clip.name = "Loop"; clip.playing_position = 1.0; clip.is_playing = True; song.tracks[0].clip_slots[0].clip = clip; song.is_playing = True
        song.capture_and_insert_scene = lambda: song.scenes.insert(1, FakeScene("Captured"))
        mapper = LiveObjectMapper(song, provenance="real-live")
        request = {"expectedStateRevision": host_capture_authority_revision(mapper.snapshot())}
        clip.playing_position = 2.5  # the clip plays on between preview and apply
        validate_operation_payload("scene.capture", "request", request)
        captured = mapper.invoke("scene.capture", request, "transaction-scene-capture")
        self.assertEqual(([scene.name for scene in song.scenes], captured["captured"]), (["Scene 1", "Captured"], True))
        clip.add_new_notes([{"pitch": 60, "start_time": 0.0, "duration": 1.0, "velocity": 100}])
        with self.assertRaisesRegex(ValueError, "changed since capture preview"): mapper.invoke("scene.capture", request, "transaction-stale-capture")


class RackStateAgreementTests(unittest.TestCase):
    def test_a_drum_racks_state_is_fenced_on_the_pads_its_row_lists(self):
        # A Drum Rack has 128 drum_pads, and its snapshot row lists the 16 Live shows: the bridge's fence
        # must be the one the host computes from that row, or every rack edit was "changed since preview".
        song = FakeSong(); rack = FakeRackDevice(); rack.name = "Drum Rack"; rack.can_have_drum_pads = True
        rack.drum_pads = [type("Pad", (), {"name": f"Pad {index}", "note": index, "chains": [], "mute": False, "solo": False})() for index in range(128)]
        rack.visible_drum_pads = rack.drum_pads[36:52]; song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(len(row["drumPads"]), 16)
        for action in ("store-variation", "randomize-macros"):
            row = mapper.snapshot()["tracks"][0]["devices"][0]
            request = {"ref": row["ref"], "action": action, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": host_rack_state_revision(row)}
            validate_operation_payload("rack.action", "request", request)
            self.assertTrue(mapper.invoke("rack.action", request)["done"])
        self.assertEqual(rack.variation_count, 2)
        # An instrument rack (no pads) agrees too.
        instrument = FakeRackDevice(); song.tracks[0].devices = [instrument]; row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertTrue(mapper.invoke("rack.set", {"ref": row["ref"], "selectedVariationIndex": 0, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": host_rack_state_revision(row)})["changed"])

    def test_a_loaded_kits_fingerprint_is_the_rack_as_the_host_expands_it(self):
        # Pads name the rack's chains (listedOnRack) on the wire; the host expands them before it
        # fingerprints, so the bridge's fingerprint must hash the same expanded row or a loaded kit never settles.
        song = FakeSong(); rack = FakeRackDevice(); rack.name = "808 Core Kit"; rack.can_have_drum_pads = True
        chain = type("Chain", (), {"name": "Kick", "devices": [FakeDevice()], "in_note": 36})(); rack.chains = [chain]
        rack.drum_pads = [type("Pad", (), {"name": "Kick", "note": 36, "chains": [chain], "mute": False, "solo": False})()]
        song.tracks[0].devices = [rack]; mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertTrue(row["drumPads"][0]["chains"][0].get("listedOnRack"))
        expanded = remote_module._expanded_pad_chains(row)
        self.assertEqual(expanded["drumPads"][0]["chains"][0], expanded["chains"][0])
        host = hashlib.sha256(mapper._bounded_canonical(_owned_device_row(expanded)).encode("utf-8")).hexdigest()
        self.assertEqual(mapper._mapped_fingerprint(row["ref"]), host)


class BrowserChainLoadTests(unittest.TestCase):
    """Browser loads into a rack's chain: a native device by name, anything else hot-swapped onto a placeholder."""
    NATIVE = {"Operator": 1, "Simpler": 1, "Utility": 2, "Reverb": 2, "Velocity": 4, "Arpeggiator": 4}

    def _setup(self, misplace=False):
        native = self.NATIVE
        class Item:
            def __init__(self, name, children=None, loadable=False): self.name = name; self.children = children or []; self.is_loadable = loadable; self.is_device = loadable
        class Chain:
            def __init__(self, name, rack): self.name = name; self.devices = []; self.mute = False; self.solo = False; self.canonical_parent = rack
            def delete_device(self, index): self.devices.pop(index)
            def insert_device(self, name, index):
                # Like Live: only a native device's name inserts.
                if name not in native: raise RuntimeError(f"unknown device {name}")
                device = FakeDevice(); device.name = name; device.type = native[name]; self.devices.insert(index if index >= 0 else len(self.devices), device)
        class View:
            def __init__(self, track): self.selected_track = track; self.selected = None
            def select_device(self, device): self.selected = device
        song = FakeSong(); track = song.tracks[0]; rack = FakeRackDevice(); rack.type = 1; filled = Chain("Filled", rack); empty = Chain("Empty", rack)
        filled.devices = [FakeDevice()]; rack.chains = [filled, empty]; track.devices = [rack]; track.delete_device = lambda index: track.devices.pop(index)
        song.view = View(track)
        class Browser:
            def __init__(self):
                self.hotswap_target = None
                self.audio_effects = Item("audio_effects", [Item("Reverb", loadable=True), Item("LFO", loadable=True)])
                self.instruments = Item("instruments", [Item("Operator", loadable=True), Item("DS Kick", loadable=True)])
                self.midi_effects = Item("midi_effects", [Item("Arpeggiator", loadable=True), Item("Expression Control", loadable=True)])
            def load_item(self, _item):
                device = FakeDevice(); device.name = _item.name; device.type = {"DS Kick": 1}.get(_item.name, 2)
                if misplace: track.devices.append(device); return
                # Hot-swap replaces its target where it is; without one, Live replaces the track's instrument.
                for chain in rack.chains:
                    if self.hotswap_target in chain.devices: chain.devices[chain.devices.index(self.hotswap_target)] = device; return
                track.devices[track.devices.index(rack)] = device
        browser = Browser(); mapper = LiveObjectMapper(song); mapper._browser = lambda: browser
        return song, track, rack, filled, empty, mapper

    def _load(self, mapper, category, name, chain_index):
        item = next(row for row in mapper.invoke("browser.search", {"category": category, "limit": 10})["items"] if row["name"] == name)
        row = mapper.snapshot()["tracks"][0]; chain = row["devices"][0]["chains"][chain_index]
        return mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "chainRef": chain["ref"], "expectedChainIdentity": chain["objectIdentity"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in chain["devices"]]})

    def test_native_devices_go_in_by_name_and_max_for_live_devices_by_hot_swap(self):
        song, track, rack, filled, empty, mapper = self._setup()
        result = self._load(mapper, "audio_effects", "Reverb", 0)
        self.assertEqual([device.name for device in filled.devices], ["Utility", "Reverb"]); self.assertEqual(track.devices, [rack])
        self.assertEqual(result["deviceRef"], next(device["ref"] for device in mapper.snapshot()["tracks"][0]["devices"][0]["chains"][0]["devices"] if device["name"] == "Reverb"))
        self._load(mapper, "audio_effects", "LFO", 0)
        self.assertEqual([device.name for device in filled.devices], ["Utility", "Reverb", "LFO"], "hot-swapped onto a placeholder at the end, which went")
        self._load(mapper, "instruments", "Operator", 1)
        self.assertEqual([device.name for device in empty.devices], ["Operator"])
        with self.assertRaisesRegex(ValueError, "already has an instrument"): self._load(mapper, "instruments", "DS Kick", 1)
        self._load(mapper, "midi_effects", "Arpeggiator", 1)
        self.assertEqual([device.name for device in empty.devices], ["Arpeggiator", "Operator"], "a MIDI effect goes before the instrument")
        with self.assertRaisesRegex(ValueError, "MIDI effect"): self._load(mapper, "midi_effects", "Expression Control", 1)
        self.assertEqual([device.name for device in empty.devices], ["Arpeggiator", "Operator"]); self.assertEqual(track.devices, [rack])

    def test_a_hot_swapped_max_for_live_instrument_fills_an_empty_chain(self):
        song, track, rack, filled, empty, mapper = self._setup()
        self._load(mapper, "instruments", "DS Kick", 1)
        self.assertEqual([device.name for device in empty.devices], ["DS Kick"]); self.assertEqual(track.devices, [rack])

    def test_a_load_live_puts_elsewhere_is_taken_away(self):
        song, track, rack, filled, empty, mapper = self._setup(misplace=True)
        with self.assertRaisesRegex(ValueError, "on the track, not in the chain; nothing was left behind"): self._load(mapper, "audio_effects", "LFO", 1)
        self.assertEqual(track.devices, [rack]); self.assertEqual(empty.devices, [], "the placeholder went too")

    def test_device_discovery_lists_a_racks_chains_empty_ones_too(self):
        song, track, rack, filled, empty, mapper = self._setup()
        rows = mapper.discover("device", requested_fields=["name", "chainList"])["items"]
        rack_row = next(row for row in rows if row["name"] == "Rack")
        self.assertEqual([chain["name"] for chain in rack_row["chainList"]], ["Filled", "Empty"])
        self.assertTrue(all(set(chain) == {"ref", "name"} for chain in rack_row["chainList"]))
        self.assertNotIn("chainList", next(row for row in rows if row["name"] == "Utility"))

    def test_a_chains_devices_changed_since_preview_refuse(self):
        song, track, rack, filled, empty, mapper = self._setup()
        item = mapper.invoke("browser.search", {"category": "audio_effects", "limit": 10})["items"][0]
        row = mapper.snapshot()["tracks"][0]; chain = row["devices"][0]["chains"][0]
        filled.devices.append(FakeDevice())
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("browser.load", {"itemId": item["id"], "trackRef": row["ref"], "chainRef": chain["ref"], "expectedChainIdentity": chain["objectIdentity"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"], "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in chain["devices"]]})


class RackMacroDrumPadTests(unittest.TestCase):
    def test_chain_rows_expose_color_io_and_drum_fields(self):
        song = FakeSong()
        chain = type("Chain", (), {"name": "Chain 1", "devices": [], "mute": False, "solo": False})()
        chain.color_index = 5; chain.is_auto_colored = True
        chain.has_audio_input = True; chain.has_audio_output = True; chain.has_midi_input = False; chain.has_midi_output = False
        chain.muted_via_solo = False; chain.in_note = 36; chain.out_note = 51; chain.choke_group = 1
        rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True; rack.chains = [chain]
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]["chains"][0]
        self.assertEqual((row["colorIndex"], row["autoColor"]), (5, True))
        self.assertEqual((row["hasAudioInput"], row["hasMidiOutput"]), (True, False))
        self.assertEqual((row["mutedViaSolo"], row["inNote"], row["outNote"], row["chokeGroup"]), (False, 36, 51, 1))

    def test_chain_set_color_and_flags_with_rollback(self):
        song = FakeSong()
        chain = type("Chain", (), {"name": "Chain 1", "devices": [], "mute": False, "solo": False})()
        chain.color_index = 1; chain.is_auto_colored = False
        rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True; rack.chains = [chain]
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]["chains"][0]
        def fences():
            state = {"colorIndex": chain.color_index, "autoColor": chain.is_auto_colored, "mute": chain.mute, "solo": chain.solo}
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("chain.set"))
        result = mapper.invoke("chain.set", {**fences(), "colorIndex": 9, "autoColor": True, "mute": True, "solo": True})
        self.assertTrue(result["changed"]); validate_operation_payload("chain.set", "result", result)
        self.assertEqual((chain.color_index, chain.is_auto_colored, chain.mute, chain.solo), (9, True, True, True))
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("chain.set", {**fences(), "colorIndex": 70})
        with self.assertRaisesRegex(ValueError, "no fields"): mapper.invoke("chain.set", fences())

    def test_drum_pad_set_and_delete_all_chains(self):
        song = FakeSong()
        pad = type("DrumPad", (), {"name": "Pad 1", "mute": False, "note": 36, "solo": False, "chains": [1, 2]})()
        pad.delete_all_chains = lambda: setattr(pad, "chains", [])
        rack = FakeDevice(); rack.name = "Drum Rack"; rack.can_have_chains = True; rack.can_have_drum_pads = True; rack.drum_pads = [pad]
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"][0]
        self.assertEqual((row["note"], row["solo"]), (36, False))
        def fences():
            state = {"note": pad.note, "solo": pad.solo}
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("drum-pad.set"))
        result = mapper.invoke("drum-pad.set", {**fences(), "solo": True})
        self.assertTrue(result["changed"]); validate_operation_payload("drum-pad.set", "result", result)
        self.assertEqual((pad.note, pad.solo), (36, True))
        with self.assertRaisesRegex(ValueError, "read-only"): mapper.invoke("drum-pad.set", {**fences(), "note": 40})
        self.assertTrue(mapper._operation_supported("drum-pad.delete-all-chains"))
        chains_revision = hashlib.sha256(mapper._bounded_canonical([mapper._capture_object_identity(chain) for chain in pad.chains]).encode()).hexdigest()
        result = mapper.invoke("drum-pad.delete-all-chains", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": chains_revision})
        self.assertEqual(result, {"deleted": 2}); self.assertEqual(pad.chains, [])

    def test_an_authority_check_reads_the_set_once_however_many_references_it_names(self):
        """A parameter names every parameter of its device; a snapshot for each made big devices take seconds."""
        song = FakeSong(); device = FakeDevice(); device.parameters = [FakeParameter() for _ in range(40)]; song.tracks[0].devices = [device]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        args = {"ref": row["parameters"][0]["ref"], "expectedOwnerRef": row["ref"], "expectedTrackRef": mapper.snapshot()["tracks"][0]["ref"], "expectedSiblings": [{"ref": parameter["ref"], "objectIdentity": parameter["objectIdentity"]} for parameter in row["parameters"]]}
        builds = []; rows = []; build = LiveObjectMapper._build_snapshot; track_row = LiveObjectMapper._track_row
        with patch.object(LiveObjectMapper, "_build_snapshot", lambda self, args=None: builds.append(1) or build(self, args)), patch.object(LiveObjectMapper, "_track_row", lambda self, *row_args: rows.append(row_args[2]) or track_row(self, *row_args)):
            shared = _authority_state_digest(mapper, args, "device.parameter.set")
        self.assertEqual(builds, [], "no whole-Set snapshot for 42 references")
        self.assertEqual(rows, [], "no track's whole row: the parameter's own row, and its siblings' identities")
        self.assertIsNone(mapper._read_cache, "and it's gone afterwards")
        alone = remote_module._reference_state_digest(mapper, args, "device.parameter.set")
        self.assertEqual(shared, alone, "the same digest as reading each reference on its own")
        # Siblings are identities the change fences itself: a sibling's value isn't what it depends on,
        # the parameter's own value is, and so is a sibling replaced.
        device.parameters[3].value = 0.75
        self.assertEqual(_authority_state_digest(mapper, args, "device.parameter.set"), shared, "a sibling's value isn't bound")
        device.parameters[0].value = 0.25
        moved = _authority_state_digest(mapper, args, "device.parameter.set"); self.assertNotEqual(moved, shared, "its own value is")
        device.parameters[3] = FakeParameter()
        self.assertNotEqual(_authority_state_digest(mapper, args, "device.parameter.set"), moved, "and a sibling replaced is")

    def test_a_drum_rack_with_sounds_on_its_pads_reads_and_edits_like_any_rack(self):
        """Live lists a Drum Rack's chains both on the rack and on their pads; that isn't a cycle."""
        class EnableableDevice(FakeDevice):
            def __init__(self):
                super().__init__()
                on = FakeParameter(); on.value = 1.0; on.quantization = 1.0
                self.parameters = [on, FakeParameter()]
            @property
            def enabled(self): return self.parameters[0].value == 1.0
            @enabled.setter
            def enabled(self, _value): pass
        song = FakeSong(); kick = EnableableDevice(); kick.name = "Kick"
        chain = type("DrumChain", (), {"name": "Kick", "devices": [kick], "mute": False, "solo": False, "in_note": 36})()
        pad = type("DrumPad", (), {"name": "Kick", "mute": False, "note": 36, "solo": False, "chains": [chain]})()
        empty = type("DrumPad", (), {"name": "Pad", "mute": False, "note": 37, "solo": False, "chains": []})()
        rack = FakeDevice(); rack.name = "Drum Rack"; rack.can_have_chains = True; rack.can_have_drum_pads = True; rack.chains = [chain]; rack.drum_pads = [pad, empty]
        song.tracks[0].devices = [rack]; mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(row["drumPads"][0]["chains"], [{"ref": row["chains"][0]["ref"], "parentRef": row["chains"][0]["parentRef"], "index": 0, "objectIdentity": row["chains"][0]["objectIdentity"], "name": "Kick", "inNote": 36, "listedOnRack": True}], "the pad names the chain the rack lists in full, so its devices go over the wire once")
        device_identity = row["chains"][0]["devices"][0]["objectIdentity"]
        self.assertEqual(AuthenticatedRemoteScript._canonical(row).count(f'"objectIdentity":"{device_identity}"'), 1, "the device isn't repeated")
        self.assertEqual([item["name"] for item in mapper._flatten_device_rows(mapper.snapshot()["tracks"][0]["devices"])], ["Drum Rack", "Kick"], "each device once")
        track_ref = mapper.discover("track")["items"][0]["ref"]; kick_row = row["chains"][0]["devices"][0]
        base = {"ref": kick_row["ref"], "expectedObjectIdentity": kick_row["objectIdentity"], "expectedOwnerRef": kick_row["parentRef"], "expectedOwnerIdentity": row["chains"][0]["objectIdentity"], "expectedSiblings": [{"ref": kick_row["ref"], "objectIdentity": kick_row["objectIdentity"]}], "expectedTrackRef": track_ref, "expectedTrackIdentity": mapper.snapshot()["tracks"][0]["objectIdentity"]}
        changed = mapper.invoke("device.enable", {**base, "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"enabled": True}).encode()).hexdigest(), "enabled": False})
        self.assertTrue(changed["changed"], "a device on a pad is found once and edited")

    def test_a_sample_goes_onto_an_empty_drum_pad_by_either_route_and_nothing_half_made_stays(self):
        def simpler():
            device = type("InsertedDevice", (), {"name": "Simpler", "class_name": "OriginalSimpler", "enabled": True, "parameters": [], "sample": None})()
            device.replace_sample = lambda path: setattr(device, "sample", type("Sample", (), {"file_path": path})())
            return device
        def kit(route):
            """A Drum Rack whose pads show the chains playing their note, as in Live."""
            song = FakeSong(); rack = FakeDevice(); rack.name = "Drum Rack"; rack.can_have_chains = True; rack.can_have_drum_pads = True; rack.chains = []
            def new_chain(in_note):
                chain = type("DrumChain", (), {"name": "Chain", "in_note": in_note, "devices": []})()
                chain.insert_device = lambda name, index: chain.devices.insert(index, simpler())
                rack.chains.append(chain); return chain
            pads = []
            for note in (36, 37, 38):
                pad = type("DrumPad", (), {"name": "Pad", "mute": False, "note": note, "solo": False, "canonical_parent": rack})()
                type(pad).chains = property(lambda self: [chain for chain in rack.chains if chain.in_note == self.note])
                pad.delete_all_chains = (lambda self: rack.chains.__setitem__(slice(None), [chain for chain in rack.chains if chain.in_note != self.note])).__get__(pad)
                pads.append(pad)
            rack.drum_pads = pads
            if route in ("chain", "stray"): rack.insert_chain = lambda index=None: new_chain(36)
            song.tracks[0].devices = [rack]
            browser = type("Browser", (), {"instruments": type("Root", (), {"children": [type("Item", (), {"name": "Simpler", "is_device": True})()]})(), "hotswap_target": None})()
            browser.load_item = lambda item: new_chain(browser.hotswap_target.note).devices.append(simpler())
            if route != "hotswap": type(browser).hotswap_target = property(lambda self: None, lambda self, value: None)
            return song, rack, pads, browser
        for route in ("hotswap", "chain"):
            song, rack, pads, browser = kit(route); mapper = LiveObjectMapper(song)
            row = mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"][2]
            with patch.object(LiveObjectMapper, "_browser", lambda self: browser):
                self.assertTrue(mapper._operation_supported("drum-pad.load-sample"))
                result = mapper.invoke("drum-pad.load-sample", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "samplePath": "/Samples/Snare.wav", "name": "Snare"})
            validate_operation_payload("drum-pad.load-sample", "result", result)
            self.assertEqual(result["route"], route); self.assertEqual(result["samplePath"], "/Samples/Snare.wav")
            self.assertEqual(len(pads[2].chains), 1); self.assertEqual(pads[2].chains[0].name, "Snare"); self.assertEqual(pads[2].chains[0].devices[0].sample.file_path, "/Samples/Snare.wav")
            self.assertEqual([len(pad.chains) for pad in pads[:2]], [0, 0], "no other pad changes")
            self.assertEqual(len(mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"][2]["chains"]), 1, "and the Set still reads, the pad's chain once")
            with self.assertRaisesRegex(ValueError, "already has a sound"), patch.object(LiveObjectMapper, "_browser", lambda self: browser):
                mapper.invoke("drum-pad.load-sample", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "samplePath": "/Samples/Other.wav"})
        # Neither route: the chain lands on another pad and can't be moved. It's removed again and the error says what happened.
        song, rack, pads, browser = kit("stray"); mapper = LiveObjectMapper(song)
        def stuck(self, value): raise AttributeError("in_note is read-only")
        row = mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"][2]
        original = rack.insert_chain
        def insert_stuck(index=None):
            chain = original(index); type(chain).in_note = property(lambda self: 36, stuck); return chain
        rack.insert_chain = insert_stuck
        with self.assertRaisesRegex(ValueError, r"drum pad load failed: browser: the pad can't be a hot-swap target; chain: AttributeError"), patch.object(LiveObjectMapper, "_browser", lambda self: browser):
            mapper.invoke("drum-pad.load-sample", {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "samplePath": "/Samples/Snare.wav"})
        self.assertEqual(rack.chains, [], "the stray chain on the first pad is gone")
        # Several pads in one request: they all load, in order, or none stays loaded.
        song, rack, pads, browser = kit("hotswap"); mapper = LiveObjectMapper(song)
        rows = mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"]
        batch = [{"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "samplePath": f"/Samples/{name}.wav", "name": name} for row, name in zip(rows, ("Kick", "Snare", "Hat"))]
        with patch.object(LiveObjectMapper, "_browser", lambda self: browser):
            self.assertTrue(mapper._operation_supported("drum-pad.load-samples"))
            result = mapper.invoke("drum-pad.load-samples", {"pads": batch})
        validate_operation_payload("drum-pad.load-samples", "result", result)
        self.assertEqual([pad.chains[0].name for pad in pads], ["Kick", "Snare", "Hat"])
        self.assertEqual([item["route"] for item in result["pads"]], ["hotswap"] * 3)
        song, rack, pads, browser = kit("hotswap"); mapper = LiveObjectMapper(song)
        rows = mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"]
        batch = [{"ref": rows[0]["ref"], "expectedObjectIdentity": rows[0]["objectIdentity"], "samplePath": "/Samples/Kick.wav"}, {"ref": rows[1]["ref"], "expectedObjectIdentity": "someone-else", "samplePath": "/Samples/Snare.wav"}]
        with self.assertRaisesRegex(ValueError, r"^drum pad 2 of 2: drum pad identity changed since preview"), patch.object(LiveObjectMapper, "_browser", lambda self: browser):
            mapper.invoke("drum-pad.load-samples", {"pads": batch})
        self.assertEqual([len(pad.chains) for pad in pads], [0, 0, 0], "the first pad is cleared again")
        # Drum Sampler: the Browser loads a preset holding the sample onto the pad, as a drop does.
        ds_song, ds_rack, ds_pads, ds_browser = kit("hotswap"); ds_mapper = LiveObjectMapper(ds_song)
        class DrumCell(FakeDevice): pass
        def drum_cell():
            device = DrumCell(); device.name = "Kick 606"; device.class_name = "DrumCell"; return device
        preset = type("Item", (), {"name": "Kick 606.adv", "is_loadable": True, "children": []})()
        folder = type("Item", (), {"name": "abc123", "children": [preset]})()
        ds_browser.user_library = type("Root", (), {"children": [type("Item", (), {"name": "Kumi", "children": [folder]})()]})()
        makes = {"device": drum_cell}
        def load_item(item):
            chain = type("DrumChain", (), {"name": "Chain", "in_note": ds_browser.hotswap_target.note, "devices": []})()
            ds_rack.chains.append(chain); chain.devices.append(makes["device"]())
        ds_browser.load_item = load_item
        ds_rows = ds_mapper.snapshot()["tracks"][0]["devices"][0]["drumPads"]
        item_id = "user_library/Kumi/abc123/Kick 606.adv"
        with patch.object(LiveObjectMapper, "_browser", lambda self: ds_browser):
            loaded = ds_mapper.invoke("drum-pad.load-sample", {"ref": ds_rows[0]["ref"], "expectedObjectIdentity": ds_rows[0]["objectIdentity"], "instrument": "Drum Sampler", "presetItemId": item_id, "name": "Kick 606"})
        validate_operation_payload("drum-pad.load-sample", "result", loaded)
        self.assertEqual(loaded["route"], "preset"); self.assertEqual(ds_pads[0].chains[0].name, "Kick 606"); self.assertEqual(ds_pads[0].chains[0].devices[0].class_name, "DrumCell")
        self.assertIsNone(ds_browser.hotswap_target, "the target is let go")
        makes["device"] = simpler
        with self.assertRaisesRegex(ValueError, r"Live made 1 chains with .* of the Drum Sampler preset"), patch.object(LiveObjectMapper, "_browser", lambda self: ds_browser):
            ds_mapper.invoke("drum-pad.load-sample", {"ref": ds_rows[1]["ref"], "expectedObjectIdentity": ds_rows[1]["objectIdentity"], "instrument": "Drum Sampler", "presetItemId": item_id})
        self.assertEqual(len(ds_pads[1].chains), 0, "a pad that didn't get a Drum Sampler is cleared again")
        with self.assertRaisesRegex(ValueError, "isn't in Live's Browser"), patch.object(LiveObjectMapper, "_browser", lambda self: ds_browser):
            ds_mapper.invoke("drum-pad.load-sample", {"ref": ds_rows[1]["ref"], "expectedObjectIdentity": ds_rows[1]["objectIdentity"], "instrument": "Drum Sampler", "presetItemId": "user_library/Kumi/abc123/Missing.adv"})
        for bad in ({"pads": [batch[0], batch[0]]}, {"pads": []}, {"pads": batch, "ref": rows[0]["ref"]}):
            with self.assertRaises(ValueError):
                mapper.invoke("drum-pad.load-samples", bad)
        self.assertEqual([len(pad.chains) for pad in pads], [0, 0, 0])

    def test_native_rack_macro_shape_and_indexed_variation(self):
        song = FakeSong(); rack = FakeRackDevice(); del rack.macros
        rack.parameters = [FakeParameter(), FakeParameter()]
        rack.variation_count = 3
        song.tracks[0].devices = [rack]; mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(len(row["macros"]), 1)
        self.assertEqual(row["macros"][0]["objectIdentity"], row["parameters"][1]["objectIdentity"])
        called = []
        rack.recall_selected_variation = lambda: called.append(rack.selected_variation_index)
        def args(index):
            return {"ref": row["ref"], "action": "recall-variation", "index": index,
                    "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": host_rack_state_revision(mapper.snapshot()["tracks"][0]["devices"][0])}
        mapper.invoke("rack.action", args(2)); self.assertEqual(called, [2])
        with self.assertRaisesRegex(ValueError, "out of range"): mapper.invoke("rack.action", args(3))
        self.assertEqual(called, [2])
        stale = args(1); rack.selected_variation_index = 0
        with self.assertRaisesRegex(ValueError, "state changed"): mapper.invoke("rack.action", stale)

    def test_modulator_fallback_search_and_resolution_share_real_items(self):
        def node(**values): return type("BrowserNode", (), values)()
        lfo = node(name="LFO", uri="query:AudioFx#LFO", is_loadable=True, children=[])
        impostor = node(name="LFO", uri="user:LFO", is_loadable=True, children=[])
        browser = node(audio_effects=node(children=[lfo, impostor]), midi_effects=node(children=[]))
        mapper = LiveObjectMapper(FakeSong())
        with patch.object(LiveObjectMapper, "_browser", lambda self: browser):
            self.assertIn("modulators", [item["name"] for item in mapper._browser_roots({})["roots"]])
            rows = mapper._browser_search({"category": "modulators"})["items"]
            self.assertEqual(len(rows), 1)
            self.assertIs(mapper._browser_find(rows[0]["id"])[0], lfo)
            browser.modulators = node(children=[])
            self.assertEqual(mapper._browser_search({"category": "modulators"})["items"], rows)
            browser.modulators = node(children=[impostor])
            self.assertIs(mapper._browser_find(rows[0]["id"])[0], impostor)
            self.assertNotEqual(mapper._browser_find(rows[0]["id"])[1]["objectIdentity"], rows[0]["objectIdentity"])
            with self.assertRaisesRegex(ValueError, "identity"):
                mapper._browser_load({"itemId": rows[0]["id"], "expectedObjectIdentity": rows[0]["objectIdentity"], "trackRef": mapper.snapshot()["tracks"][0]["ref"]})

    def test_rack_rows_actions_and_view(self):
        song = FakeSong()
        rack = FakeRackDevice()
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(row["visibleMacroCount"], 8); self.assertEqual(row["variationCount"], 1); self.assertEqual(row["selectedVariationIndex"], 0)
        self.assertEqual(row["macrosMapped"], [True]); self.assertEqual(row["view"]["showChainDevices"], True)
        def fences(): return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._rack_state(rack)).encode()).hexdigest()}
        self.assertTrue(mapper._operation_supported("rack.set"))
        result = mapper.invoke("rack.set", {**fences(), "selectedVariationIndex": 0})
        self.assertTrue(result["changed"])
        with self.assertRaisesRegex(ValueError, "read-only"): mapper.invoke("rack.set", {**fences(), "visibleMacroCount": 16})
        self.assertTrue(mapper._operation_supported("rack.action"))
        before = rack.visible_macro_count
        result = mapper.invoke("rack.action", {**fences(), "action": "add-macro"})
        self.assertTrue(result["done"]); self.assertEqual(rack.visible_macro_count, before + 1)
        result = mapper.invoke("rack.action", {**fences(), "action": "remove-macro"})
        self.assertTrue(result["done"]); self.assertEqual(rack.visible_macro_count, before)
        result = mapper.invoke("rack.action", {**fences(), "action": "insert-chain"})
        self.assertTrue(result["done"]); self.assertEqual(len(rack.chains), 1)
        # The new chain comes back as the snapshot names it, for a device to go into next.
        chain_row = next(device for device in mapper.snapshot()["tracks"][0]["devices"] if device.get("chains"))["chains"][0]
        self.assertEqual(result["chainRef"], chain_row["ref"]); self.assertEqual(result["chainObjectIdentity"], chain_row["objectIdentity"])
        result = mapper.invoke("rack.action", {**fences(), "action": "store-variation"})
        self.assertTrue(result["done"]); self.assertEqual(rack.variation_count, 2)
        result = mapper.invoke("rack.action", {**fences(), "action": "recall-variation"})
        self.assertTrue(result["done"])
        result = mapper.invoke("rack.action", {**fences(), "action": "delete-variation"})
        self.assertTrue(result["done"]); self.assertEqual(rack.variation_count, 1)
        self.assertTrue(mapper._operation_supported("rack.view.set"))
        view_revision = hashlib.sha256(mapper._bounded_canonical({"padScrollPosition": 0, "showChainDevices": True}).encode()).hexdigest()
        result = mapper.invoke("rack.view.set", {"ref": row["ref"], "padScrollPosition": 4, "showChainDevices": False, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": view_revision})
        self.assertTrue(result["changed"]); validate_operation_payload("rack.view.set", "result", result)
        self.assertEqual(rack.view.drum_pads_scroll_position, 4); self.assertFalse(rack.view.is_showing_chain_devices)


class SpecializedDeviceTests(unittest.TestCase):
    def test_drift_set_with_rows_and_rollback(self):
        song = FakeSong()
        device = FakeDevice(); device.name = "Drift"; device.class_name = "DriftDevice"
        device.pitch_bend_range = 12; device.voice_count_index = 2; device.voice_mode_index = 0
        device.voice_count_list = [{"name": "1"}, {"name": "4"}, {"name": "8"}, {"name": "16"}]; device.voice_mode_list = [{"name": "Poly"}, {"name": "Mono"}]
        device.mod_matrix_sources = [{"name": "LFO 1"}, {"name": "LFO 2"}]; device.mod_matrix_targets = [{"name": "Pitch"}, {"name": "Filter"}]
        song.tracks[0].devices = [device]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(row["drift"]["modSources"], ["LFO 1", "LFO 2"]); self.assertEqual(row["drift"]["modTargets"], ["Pitch", "Filter"])
        self.assertEqual((row["drift"]["voiceCount"], row["drift"]["voiceCountList"]), (2, ["1", "4", "8", "16"]))
        self.assertTrue(mapper._operation_supported("drift.set"))
        def fences():
            # As the host fences a family: every field drift.set can set, as the device row shows it.
            drift = mapper.get(row["ref"])["drift"]
            state = {field: drift.get(field) for field in ("pitchBendRange", "voiceCount", "voiceMode", *LiveObjectMapper._DRIFT_MOD_FIELDS)}
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        result = mapper.invoke("drift.set", {**fences(), "pitchBendRange": 24, "voiceCount": 3, "voiceMode": 1})
        self.assertTrue(result["changed"]); validate_operation_payload("drift.set", "result", result)
        self.assertEqual((device.pitch_bend_range, device.voice_count_index, device.voice_mode_index), (24, 3, 1))
        with self.assertRaisesRegex(ValueError, "is invalid"): mapper.invoke("drift.set", {**fences(), "pitchBendRange": 200})

    def test_drum_cell_eq8_meld_sets(self):
        song = FakeSong()
        cell = FakeDevice(); cell.name = "Cell"; cell.class_name = "DrumCellDevice"; cell.gain = -6.0
        eq = FakeDevice(); eq.name = "EQ8"; eq.class_name = "Eq8Device"; eq.edit_mode = 0; eq.global_mode = 1; eq.oversample = False
        eq.view = type("Eq8View", (), {"selected_band": 2})()
        meld = FakeDevice(); meld.name = "Meld"; meld.class_name = "MeldDevice"; meld.selected_engine = 0; meld.unison_voices = 1; meld.mono_poly = False; meld.poly_voices = 8
        song.tracks[0].devices = [cell, eq, meld]
        mapper = LiveObjectMapper(song)
        rows = mapper.snapshot()["tracks"][0]["devices"]
        self.assertTrue(mapper._operation_supported("drum-cell.set")); self.assertTrue(mapper._operation_supported("eq8.set")); self.assertTrue(mapper._operation_supported("meld.set"))
        result = mapper.invoke("drum-cell.set", {"ref": rows[0]["ref"], "gain": -12.0, "expectedObjectIdentity": rows[0]["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"gain": -6.0}).encode()).hexdigest()})
        self.assertTrue(result["changed"]); self.assertEqual(cell.gain, -12.0)
        eq_state = mapper._specialized_state(eq, [("editMode", "edit_mode"), ("globalMode", "global_mode"), ("oversample", "oversample"), ("selectedBand", "view.selected_band")])
        result = mapper.invoke("eq8.set", {"ref": rows[1]["ref"], "editMode": 1, "oversample": True, "selectedBand": 4, "expectedObjectIdentity": rows[1]["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(eq_state).encode()).hexdigest()})
        self.assertTrue(result["changed"]); self.assertEqual((eq.edit_mode, eq.oversample, eq.view.selected_band), (1, True, 4))
        meld_state = mapper._specialized_state(meld, [("engine", "selected_engine"), ("unison", "unison_voices"), ("monoPoly", "mono_poly"), ("polyphony", "poly_voices")])
        result = mapper.invoke("meld.set", {"ref": rows[2]["ref"], "engine": 1, "unison": 4, "monoPoly": True, "polyphony": 16, "expectedObjectIdentity": rows[2]["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(meld_state).encode()).hexdigest()})
        self.assertTrue(result["changed"]); self.assertEqual((meld.selected_engine, meld.unison_voices, meld.mono_poly, meld.poly_voices), (1, 4, True, 16))

    def test_hybrid_reverb_ir_and_time_shaping(self):
        song = FakeSong()
        device = FakeDevice(); device.name = "Hybrid"; device.class_name = "HybridReverbDevice"
        device.ir_category_list = [{"name": "Halls"}, {"name": "Plates"}]; device.ir_category_index = 0
        device.ir_file_list = [{"name": "Hall A"}, {"name": "Hall B"}]; device.ir_file_index = 0
        device.ir_attack_time = 10.0; device.ir_decay_time = 1200.0; device.ir_size_factor = 50.0
        song.tracks[0].devices = [device]
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("hybrid-reverb.set"))
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual((row["hybridReverb"]["irCategory"], row["hybridReverb"]["irFile"], row["hybridReverb"]["attack"]), ("Halls", "Hall A", 10.0))
        identity_args = {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"]}
        result = mapper.invoke("hybrid-reverb.set", {**identity_args, "irCategory": "Plates", "irFile": "Hall B"})
        self.assertTrue(result["changed"]); self.assertEqual((device.ir_category_index, device.ir_file_index), (1, 1))
        with self.assertRaisesRegex(ValueError, "not an available choice"): mapper.invoke("hybrid-reverb.set", {**identity_args, "irCategory": "Bogus"})
        state = mapper._specialized_state(device, [("attack", "ir_attack_time"), ("decay", "ir_decay_time"), ("size", "ir_size_factor")])
        result = mapper.invoke("hybrid-reverb.set", {**identity_args, "attack": 25.0, "decay": 2400.0, "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()})
        self.assertTrue(result["changed"]); self.assertEqual((device.ir_attack_time, device.ir_decay_time), (25.0, 2400.0))
        with self.assertRaisesRegex(ValueError, "authority is invalid"): mapper.invoke("hybrid-reverb.set", {**identity_args, "time": 3000.0})

    def test_hybrid_reverb_second_phase_failure_rolls_back_applied_ir_indices(self):
        class AttackRefusingDevice(FakeDevice):
            @property
            def ir_attack_time(self): return self._ir_attack_time
            @ir_attack_time.setter
            def ir_attack_time(self, value):
                if value != self._ir_attack_time: raise RuntimeError("attack write rejected")
        device = AttackRefusingDevice(); device._ir_attack_time = 10.0
        device.name = "Hybrid"; device.class_name = "HybridReverbDevice"
        device.ir_category_list = [{"name": "Halls"}, {"name": "Plates"}]; device.ir_category_index = 0
        device.ir_file_list = [{"name": "Hall A"}, {"name": "Hall B"}]; device.ir_file_index = 0
        device.ir_decay_time = 1200.0; device.ir_size_factor = 50.0
        song = FakeSong(); song.tracks[0].devices = [device]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        specs = [("attack", "ir_attack_time"), ("decay", "ir_decay_time"), ("size", "ir_size_factor")]
        def fences():
            state = mapper._specialized_state(device, specs)
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        combined = mapper.invoke("hybrid-reverb.set", {**fences(), "irCategory": "Plates", "decay": 2400.0})
        self.assertTrue(combined["changed"]); validate_operation_payload("hybrid-reverb.set", "result", combined)
        self.assertEqual((device.ir_category_index, device.ir_decay_time), (1, 2400.0))
        with self.assertRaisesRegex(RuntimeError, "attack write rejected"):
            mapper.invoke("hybrid-reverb.set", {**fences(), "irFile": "Hall B", "attack": 25.0})
        self.assertEqual((device.ir_category_index, device.ir_file_index), (1, 0))
        self.assertEqual((device.ir_attack_time, device.ir_decay_time, device.ir_size_factor), (10.0, 2400.0, 50.0))

    def test_looper_actions_and_properties(self):
        song = FakeSong()
        device = FakeDevice(); device.name = "Looper"; device.class_name = "LooperDevice"
        device.loop_length = 4.0; device.tempo = 120.0; device.state = 0
        device.overdub_after_record = False; device.record_length_index = 0; device.record_length_list = [{"name": "1 bar"}, {"name": "2 bars"}]
        calls = []
        for name in ("record", "overdub", "play", "stop", "clear", "undo", "double_speed", "half_speed"):
            setattr(device, name, (lambda n: (lambda: calls.append(n)))(name))
        slot = song.tracks[0].clip_slots[0]
        def export_to_clip_slot(target):
            if getattr(target, "clip", None) is not None: raise RuntimeError("slot is not empty")
            target.clip = FakeClip(4.0); calls.append("export")
        device.export_to_clip_slot = export_to_clip_slot
        song.tracks[0].devices = [device]
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("looper.action")); self.assertTrue(mapper._operation_supported("looper.set"))
        row = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual((row["looper"]["loopLength"], row["looper"]["overdubAfterRecord"], row["looper"]["recordLengthIndex"]), (4.0, False, 0))
        slot_ref = mapper.snapshot()["tracks"][0]["clipSlots"][0]["ref"]
        def fences():
            state = mapper._looper_state(device)
            return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        for action in ("record", "overdub", "play", "stop", "clear", "undo", "double-speed", "half-speed"):
            result = mapper.invoke("looper.action", {**fences(), "action": action})
            self.assertTrue(result["done"]); validate_operation_payload("looper.action", "result", result)
        self.assertEqual(calls, ["record", "overdub", "play", "stop", "clear", "undo", "double_speed", "half_speed"])
        with self.assertRaisesRegex(ValueError, "exact target clip slot"): mapper.invoke("looper.action", {**fences(), "action": "export"})
        result = mapper.invoke("looper.action", {**fences(), "action": "export", "slotRef": slot_ref})
        self.assertTrue(result["done"]); self.assertEqual(calls[-1], "export"); self.assertIsNotNone(slot.clip)
        with self.assertRaisesRegex(ValueError, "not empty"): mapper.invoke("looper.action", {**fences(), "action": "export", "slotRef": slot_ref})
        prop_state = mapper._specialized_state(device, [("overdubAfterRecord", "overdub_after_record"), ("recordLengthIndex", "record_length_index")])
        result = mapper.invoke("looper.set", {**fences(), "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(prop_state).encode()).hexdigest(), "overdubAfterRecord": True, "recordLengthIndex": 1})
        self.assertTrue(result["changed"]); self.assertEqual((device.overdub_after_record, device.record_length_index), (True, 1))
        with self.assertRaisesRegex(ValueError, "not writable"): mapper.invoke("looper.set", {**fences(), "loopLength": 8.0})

    def test_plugin_presets_editor_and_simpler_replace(self):
        song = FakeSong()
        plugin = FakeDevice(); plugin.name = "Plugin"; plugin.class_name = "PluginDevice"
        plugin.presets = ["Init", "Warm", "Bright"]; plugin.selected_preset_index = 0; plugin.is_editor_open = False
        simpler = FakeDevice(); simpler.name = "Simpler"; simpler.class_name = "SimplerDevice"
        simpler.sample = type("Sample", (), {"file_path": "/old/a.wav"})()
        def replace_sample(path): simpler.sample.file_path = path
        simpler.replace_sample = replace_sample
        song.tracks[0].devices = [plugin, simpler]
        mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("plugin.set")); self.assertTrue(mapper._operation_supported("simpler.replace-sample"))
        rows = mapper.snapshot()["tracks"][0]["devices"]
        self.assertEqual(rows[0]["plugin"]["presets"], ["Init", "Warm", "Bright"]); self.assertFalse(rows[0]["plugin"]["isEditorOpen"])
        state = {"presetIndex": 0, "isEditorOpen": False}
        result = mapper.invoke("plugin.set", {"ref": rows[0]["ref"], "presetIndex": 2, "isEditorOpen": True, "expectedObjectIdentity": rows[0]["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()})
        self.assertTrue(result["changed"]); self.assertEqual((plugin.selected_preset_index, plugin.is_editor_open), (2, True))
        with self.assertRaisesRegex(ValueError, "exceeds"): mapper.invoke("plugin.set", {"ref": rows[0]["ref"], "presetIndex": 99, "expectedObjectIdentity": rows[0]["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical({"presetIndex": 2, "isEditorOpen": True}).encode()).hexdigest()})
        sample_revision = hashlib.sha256(mapper._bounded_canonical({"filePath": "/old/a.wav"}).encode()).hexdigest()
        result = mapper.invoke("simpler.replace-sample", {"ref": rows[1]["ref"], "filePath": "/new/b.wav", "expectedObjectIdentity": rows[1]["objectIdentity"], "expectedStateRevision": sample_revision})
        self.assertTrue(result["changed"]); self.assertEqual(simpler.sample.file_path, "/new/b.wav")
        with self.assertRaisesRegex(ValueError, "absolute path"): mapper.invoke("simpler.replace-sample", {"ref": rows[1]["ref"], "filePath": "b.wav", "expectedObjectIdentity": rows[1]["objectIdentity"], "expectedStateRevision": sample_revision})


class ObserverModelTests(unittest.TestCase):
    def test_subscribe_poll_unsubscribe_with_dedup_and_revision_context(self):
        song = FakeSong()
        song.tuning_system = FakeTuningSystem()
        song.root_note = 0; song.scale_name = "Major"; song.scale_mode = "Ionian"; song.scale_intervals = [0, 2, 4, 5, 7, 9, 11]
        song.groove_pool = FakeGroovePool(); song.groove_amount = 0.0
        clip = FakeClip(4.0); clip.add_new_notes([{"pitch": 60, "start_time": 0.0, "duration": 0.5, "velocity": 100}])
        song.tracks[0].clip_slots[0].clip = clip
        scene = song.scenes[0]; scene.color_index = 1; scene.tempo = 120.0
        mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        track_ref = snapshot["tracks"][0]["ref"]; clip_ref = snapshot["tracks"][0]["clips"][0]["ref"]; scene_ref = snapshot["scenes"][0]["ref"]
        result = mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}, {"kind": "selection"}, {"kind": "track", "ref": track_ref}, {"kind": "clip", "ref": clip_ref}, {"kind": "groove"}, {"kind": "tuning"}, {"kind": "scene", "ref": scene_ref}], "minIntervalMs": 100})
        self.assertTrue(result["subscriptionId"].startswith("obs_")); self.assertEqual(len(result["topics"]), 7)
        self.assertEqual(len(result["revisions"]), 7); validate_operation_payload("observe.subscribe", "result", result)
        subscription_id = result["subscriptionId"]
        import time as _time
        _time.sleep(0.11)
        quiet = mapper.invoke("observe.poll", {"subscriptionId": subscription_id})
        self.assertEqual(quiet["events"], []); self.assertFalse(quiet["overflow"]); self.assertEqual(quiet["sequence"], 1)
        song.is_playing = True; clip.loop_start = 1.0; song.groove_amount = 0.5
        _time.sleep(0.11)
        changed = mapper.invoke("observe.poll", {"subscriptionId": subscription_id})
        kinds = {event["kind"] for event in changed["events"]}
        self.assertIn("transport", kinds); self.assertIn("clip", kinds); self.assertIn("groove", kinds)
        self.assertNotIn("selection", kinds); self.assertNotIn("tuning", kinds)
        for event in changed["events"]:
            self.assertEqual(len(event["revision"]), 64); self.assertIsInstance(event["changedFields"], list)
        validate_operation_payload("observe.poll", "result", changed)
        _time.sleep(0.11)
        quiet_again = mapper.invoke("observe.poll", {"subscriptionId": subscription_id})
        self.assertEqual(quiet_again["events"], [])
        with self.assertRaisesRegex(ValueError, "minimum interval"):
            mapper.invoke("observe.poll", {"subscriptionId": subscription_id})
        result = mapper.invoke("observe.unsubscribe", {"subscriptionId": subscription_id})
        self.assertEqual(result, {"unsubscribed": True}); validate_operation_payload("observe.unsubscribe", "result", result)
        with self.assertRaisesRegex(ValueError, "unknown or expired"): mapper.invoke("observe.poll", {"subscriptionId": subscription_id})

    def test_observe_quotas_and_overflow(self):
        song = FakeSong(); mapper = LiveObjectMapper(song)
        track_ref = mapper.snapshot()["tracks"][0]["ref"]
        topics = [{"kind": "meters", "ref": track_ref}] * 2
        with self.assertRaisesRegex(ValueError, "duplicate"): mapper.invoke("observe.subscribe", {"topics": topics})
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("observe.subscribe", {"topics": [{"kind": "bogus"}]})
        with self.assertRaisesRegex(ValueError, "invalid"): mapper.invoke("observe.subscribe", {"topics": []})
        with self.assertRaisesRegex(ValueError, "quota is exhausted"):
            for _ in range(9):
                mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})

    def test_expired_subscriptions_are_swept_before_the_quota_check(self):
        song = FakeSong(); mapper = LiveObjectMapper(song)
        for _ in range(8):
            mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})
        with self.assertRaisesRegex(ValueError, "quota is exhausted"):
            mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})
        for subscription in mapper._observe_subscriptions.values():
            subscription["expiresAtMs"] = 0
        result = mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})
        self.assertTrue(result["subscriptionId"].startswith("obs_")); validate_operation_payload("observe.subscribe", "result", result)
        self.assertEqual(len(mapper._observe_subscriptions), 1)

    def test_failing_topic_digest_does_not_renew_the_subscription(self):
        song = FakeSong(); mapper = LiveObjectMapper(song)
        track_ref = mapper.snapshot()["tracks"][0]["ref"]
        result = mapper.invoke("observe.subscribe", {"topics": [{"kind": "track", "ref": track_ref}], "minIntervalMs": 100})
        subscription = mapper._observe_subscriptions[result["subscriptionId"]]
        mapper.refs.delete(track_ref)
        before = subscription["expiresAtMs"]
        with self.assertRaises(KeyError):
            mapper.invoke("observe.poll", {"subscriptionId": result["subscriptionId"]})
        self.assertEqual(subscription["expiresAtMs"], before)

    def test_reconnect_clears_prior_epoch_subscriptions(self):
        song = FakeSong(); mapper = LiveObjectMapper(song)
        for _ in range(8):
            mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})
        self.assertEqual(len(mapper._observe_subscriptions), 8)
        mapper.invoke("session.reconnect", {})
        self.assertEqual(mapper._observe_subscriptions, {})
        result = mapper.invoke("observe.subscribe", {"topics": [{"kind": "transport"}], "minIntervalMs": 100})
        self.assertTrue(result["subscriptionId"].startswith("obs_"))


class BrowserSurfaceTests(unittest.TestCase):
    def test_browser_roots_reports_unofficial_internal_bindings(self):
        class Item:
            def __init__(self, name, children=None): self.name = name; self.children = children or []
        class Browser:
            instruments = Item("instruments", [Item("Operator")])
            sounds = Item("sounds", [Item("Bass")])
            samples = Item("samples", [Item("Kick.wav")])
            legacy_libraries = Item("legacy", [])
            tunings = Item("tunings", [])
            def preview_item(self, item): pass
        mapper = LiveObjectMapper(FakeSong()); mapper._browser = lambda: Browser()
        self.assertTrue(mapper._operation_supported("browser.roots"))
        result = mapper.invoke("browser.roots", {})
        names = {root["name"]: (root["binding"], root["searchable"]) for root in result["roots"]}
        self.assertEqual(names["instruments"], ("unofficial-internal", True))
        self.assertIn("undocumented Remote Script internals", result["bindingEvidence"])
        self.assertEqual(names["sounds"], ("unofficial-internal", True))
        self.assertEqual(names["samples"], ("unofficial-internal", True))
        self.assertEqual(names["legacy_libraries"], ("unofficial-internal", False))
        self.assertEqual(names["tunings"], ("unofficial-internal", False))
        self.assertTrue(result["previewAvailable"])
        validate_operation_payload("browser.roots", "result", result)
        search = mapper.invoke("browser.search", {"category": "sounds", "limit": 10})
        self.assertEqual([item["name"] for item in search["items"]], ["Bass"])

    def test_browser_preview_item_declined_by_design(self):
        registry, _ = operation_registry()
        preview_ops = [item["id"] for item in registry["operations"] if item["id"] in {"browser.preview.start", "browser.preview.stop"}]
        self.assertEqual(sorted(preview_ops), ["browser.preview.start", "browser.preview.stop"])
        mapper = LiveObjectMapper(FakeSong())
        self.assertFalse(mapper._operation_supported("browser.preview.start"))
        self.assertFalse(mapper._operation_supported("browser.preview.stop"))


class LetteredReturnTrack(FakeTrack):
    """Behaves like Live: a return track shows its letter and prepends it to any name it is given."""

    def __init__(self, song, stored):
        super().__init__()
        self._song, self._stored, self.reject = song, stored, set()

    @property
    def name(self):
        return f"{chr(ord('A') + self._song.return_tracks.index(self))}-{self._stored}"

    @name.setter
    def name(self, value):
        if value in getattr(self, "reject", ()): raise RuntimeError("Live refused the name")
        self._stored = value


class ReturnTrackNamingTests(unittest.TestCase):
    def test_rename_accepts_bare_or_displayed_names_without_doubling_the_letter(self):
        song = FakeSong(); verb = LetteredReturnTrack(song, "Reverb"); song.return_tracks = [verb]
        mapper = LiveObjectMapper(song)

        def rename(name):
            row = next(row for row in mapper.snapshot()["tracks"] if row["kind"] == "return")
            return mapper.invoke("track.rename", {"ref": row["ref"], "name": name, "expectedName": row["name"], "expectedObjectIdentity": row["objectIdentity"],
                                                  "expectedAuthorityRevision": mapper._rename_authority_revision("track", row["ref"])})

        self.assertEqual(verb.name, "A-Reverb")
        self.assertEqual(rename("Kumi Space")["name"], "A-Kumi Space")
        self.assertEqual(rename("A-Hall")["name"], "A-Hall")
        self.assertEqual(verb._stored, "Hall")
        verb.reject = {"Broken"}
        with self.assertRaisesRegex(ValueError, "postcondition was not confirmed"): rename("Broken")
        self.assertEqual(verb.name, "A-Hall", "rollback restores the exact displayed name, not A-A-Hall")
        with self.assertRaisesRegex(ValueError, "name is invalid"): rename("A-")

    def test_regular_track_rename_is_unchanged(self):
        song = FakeSong(); mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        result = mapper.invoke("track.rename", {"ref": row["ref"], "name": "A-Bass", "expectedName": row["name"], "expectedObjectIdentity": row["objectIdentity"],
                                                "expectedAuthorityRevision": mapper._rename_authority_revision("track", row["ref"])})
        self.assertEqual((result["name"], song.tracks[0].name), ("A-Bass", "A-Bass"))

    def test_return_creation_names_with_the_letter_and_removes_a_return_it_cannot_name(self):
        song = FakeSong(); song.return_tracks = [LetteredReturnTrack(song, "Reverb")]

        def create(reject=()):
            def create_return_track():
                track = LetteredReturnTrack(song, "Return"); track.reject = set(reject); song.return_tracks.append(track); return track
            return create_return_track

        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        mapper = LiveObjectMapper(song)
        song.create_return_track = create()
        result = mapper.invoke("track.create-return", {"name": "Verb", "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual((result["name"], result["index"]), ("B-Verb", 1)); validate_operation_payload("track.create-return", "result", result)
        self.assertEqual(mapper.invoke("track.create-return", {"name": "C-Plate", "expectedStructureRevision": mapper._structure_revision()})["name"], "C-Plate")
        song.create_return_track = create(reject={"Delay"})
        with self.assertRaisesRegex(ValueError, "was removed"):
            mapper.invoke("track.create-return", {"name": "Delay", "expectedStructureRevision": mapper._structure_revision()})
        self.assertEqual([track.name for track in song.return_tracks], ["A-Reverb", "B-Verb", "C-Plate"])


class ReturnTrackOwnershipTests(unittest.TestCase):
    def test_created_return_is_discoverable_and_removable_by_its_creator_under_real_live_rules(self):
        song = FakeSong(); song.return_tracks = [LetteredReturnTrack(song, "Reverb")]

        def create_return_track():
            track = LetteredReturnTrack(song, "Return"); song.return_tracks.append(track); return track

        song.create_return_track = create_return_track
        song.delete_return_track = lambda index: song.return_tracks.pop(index)
        mapper = LiveObjectMapper(song, provenance="real-live")
        created = mapper.invoke("track.create-return", {"name": "Kumi Verb", "expectedStructureRevision": mapper._structure_revision()}, "transaction-0001")
        self.assertEqual({row["ref"]: row["name"] for row in mapper.snapshot()["tracks"]}.get(created["ref"]), "B-Kumi Verb")
        self.assertEqual(mapper.get(created["ref"])["name"], "B-Kumi Verb")
        deleted = mapper.invoke("track.delete-return", {"ref": created["ref"], "expectedObjectIdentity": created["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}, "transaction-0001", created["ownershipToken"])
        self.assertEqual(deleted, {"deleted": created["ref"]})
        self.assertEqual([track.name for track in song.return_tracks], ["A-Reverb"])


class Float32Parameter(FakeParameter):
    """Like Live: a parameter stores its value as a 32-bit float."""

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, new):
        import struct
        self._value = struct.unpack("f", struct.pack("f", float(new)))[0]


class Float32ConfirmationTests(unittest.TestCase):
    def test_mixer_confirms_values_that_live_rounds_to_32_bit_floats(self):
        song = FakeSong(); track = song.tracks[0]; track.mute = False; track.solo = False
        volume, pan, cue, send = Float32Parameter(), Float32Parameter(), Float32Parameter(), Float32Parameter(); volume.value = 0.5; pan.value = 0.0; cue.value = 0.7; send.value = 0.1
        track.mixer_device = type("Mixer", (), {"volume": volume, "panning": pan, "cue_volume": cue, "sends": [send]})()
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]; mixer = row["mixer"]
        state = {field: mixer.get(field) for field in ("volume", "pan", "mute", "solo", "cueVolume", "sends")}
        args = {"ref": row["ref"], "volume": 0.6, "pan": -0.25, "sends": [0.3], "expectedObjectIdentity": row["objectIdentity"], "expectedVolumeIdentity": mixer["volumeIdentity"], "expectedPanIdentity": mixer["panIdentity"],
                "expectedCueIdentity": mixer["cueIdentity"], "expectedSendIdentities": mixer["sendIdentities"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        mapper.invoke("mixer.set", args)
        self.assertNotEqual(volume.value, 0.6, "the fake really rounds like Live")
        self.assertAlmostEqual(volume.value, 0.6, places=6); self.assertAlmostEqual(send.value, 0.3, places=6)

    def test_song_swing_that_live_rounds_to_a_32_bit_float_is_confirmed(self):
        class Float32SwingSong(FakeSong):
            @property
            def swing_amount(self): return self.__dict__.get("_swing", 0.0)
            @swing_amount.setter
            def swing_amount(self, value): self.__dict__["_swing"] = struct.unpack("f", struct.pack("f", float(value)))[0]
        song = Float32SwingSong(); song.signature_numerator = 4; song.signature_denominator = 4; song.swing_amount = 0.0; song.clip_trigger_quantization = 4; song.midi_recording_quantization = 0
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["set"]
        request = {"setRef": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(mapper._song_settings_state()).encode()).hexdigest(), "swingAmount": 0.15}
        result = mapper.invoke("song.set", request); validate_operation_payload("song.set", "result", result)
        self.assertNotEqual(song.swing_amount, 0.15, "the fake really rounds like Live"); self.assertAlmostEqual(song.swing_amount, 0.15, places=6)
        # A whole-number setting Live doesn't take is still refused exactly, and the swing rolls back.
        class RefusingNumeratorSong(Float32SwingSong):
            @property
            def signature_numerator(self): return 4
            @signature_numerator.setter
            def signature_numerator(self, value): pass
        refusing = RefusingNumeratorSong(); refusing.signature_denominator = 4; refusing.swing_amount = 0.0; refusing.clip_trigger_quantization = 4; refusing.midi_recording_quantization = 0
        other = LiveObjectMapper(refusing); row = other.snapshot()["set"]
        with self.assertRaisesRegex(ValueError, "song settings change was not confirmed"):
            other.invoke("song.set", {"setRef": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(other._bounded_canonical(other._song_settings_state()).encode()).hexdigest(), "swingAmount": 0.15, "signatureNumerator": 7})
        self.assertEqual(refusing.swing_amount, 0.0)


class DeviceOwnershipFingerprintTests(unittest.TestCase):
    def test_a_reverted_parameter_tweak_keeps_a_created_device_fingerprint(self):
        song = FakeSong(); song.tracks[0].devices = [FakeDevice()]; mapper = LiveObjectMapper(song)
        device = mapper.snapshot()["tracks"][0]["devices"][0]; parameter = device["parameters"][0]
        created = mapper._ownership_fingerprint(device["ref"])
        mapper._set_parameter_value(parameter["ref"], 0.75); mapper._set_parameter_value(parameter["ref"], 0.5)
        self.assertNotEqual(mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]["revision"], parameter["revision"], "the edit counter moved")
        self.assertEqual(mapper._ownership_fingerprint(device["ref"]), created)
        mapper._set_parameter_value(parameter["ref"], 0.75)
        self.assertNotEqual(mapper._ownership_fingerprint(device["ref"]), created, "a real change still counts")

    def test_other_tracks_arriving_keeps_a_created_track_fingerprint(self):
        song = FakeSong(); track = song.tracks[0]
        track.available_input_routing_types = ["Ext. In", "Resampling"]; track.input_routing_type = type("Route", (), {"display_name": "Ext. In"})()
        mapper = LiveObjectMapper(song); reference = mapper.snapshot()["tracks"][0]["ref"]
        created = mapper._ownership_fingerprint(reference)
        track.available_input_routing_types = ["Ext. In", "Resampling", "2-MIDI"]
        self.assertEqual(mapper._ownership_fingerprint(reference), created, "a new track elsewhere is a new routing choice, not a change to this track")
        track.input_routing_type = type("Route", (), {"display_name": "Resampling"})()
        self.assertNotEqual(mapper._ownership_fingerprint(reference), created, "choosing another input still counts")

    def test_rack_view_is_not_device_content(self):
        rack = {"ref": "1:device:0:0", "canHaveChains": True, "view": {"selectedPadIndex": 1}, "parameters": [{"ref": "p", "value": 0.5, "revision": 4}],
                "chains": [{"devices": [{"canHaveChains": False, "parameters": [{"value": 1.0, "revision": 9}]}]}]}
        self.assertEqual(_owned_device_row(rack), {"ref": "1:device:0:0", "canHaveChains": True, "parameters": [{"ref": "p", "value": 0.5}],
                                                   "chains": [{"devices": [{"canHaveChains": False, "parameters": [{"value": 1.0}]}]}]})


class LazyPlayheadTests(unittest.TestCase):
    """Live applies playhead moves on its next tick; nothing may treat that as a failure."""

    def lazy_song(self):
        song = FakeArrangementSong()
        state = {"time": float(song.current_song_time), "pending": None}
        cls = type("LazyArrangementSong", (type(song),), {
            "current_song_time": property(lambda self: state["time"], lambda self, value: state.__setitem__("pending", float(value))),
        })
        song.__class__ = cls
        song.tick = lambda: state.update(time=state["pending"] if state["pending"] is not None else state["time"], pending=None)
        return song

    def test_transport_position_is_accepted_before_live_applies_it(self):
        song = self.lazy_song(); mapper = LiveObjectMapper(song)
        snapshot = mapper.snapshot()
        result = mapper.invoke("transport.set", {"setRef": snapshot["set"]["ref"], "expectedObjectIdentity": snapshot["set"]["objectIdentity"], "expectedRevision": snapshot["playback"]["revision"], "position": 12.0})
        self.assertTrue(result["changed"])
        song.tick(); self.assertEqual(song.current_song_time, 12.0)

    def test_locator_creation_moves_the_playhead_then_succeeds_on_retry(self):
        song = self.lazy_song(); mapper = LiveObjectMapper(song)
        create_args = {"name": "Drop", "position": 16, "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]}
        with self.assertRaisesRegex(ValueError, "playhead is moving; retry shortly"):
            mapper.invoke("arrangement.locator.create", create_args)
        self.assertEqual([item["name"] for item in mapper.discover("locator")["items"]], ["Intro"], "nothing created before the playhead lands")
        song.tick()
        created = mapper.invoke("arrangement.locator.create", create_args)
        self.assertEqual((created["name"], created["position"]), ("Drop", 16.0))
        # Deleting a locator away from the playhead follows the same move-then-retry protocol.
        intro = mapper.discover("locator")["items"][0]
        delete_args = {"ref": intro["ref"], "expectedObjectIdentity": intro["objectIdentity"], "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]}
        with self.assertRaisesRegex(ValueError, "playhead is moving; retry shortly"):
            mapper.invoke("arrangement.locator.delete", delete_args)
        song.tick()
        self.assertEqual(mapper.invoke("arrangement.locator.delete", delete_args), {"deleted": intro["ref"]})
        self.assertEqual([item["name"] for item in mapper.discover("locator")["items"]], ["Drop"])


class LocatorPastTheEndTests(unittest.TestCase):
    def test_a_locator_past_the_end_of_the_set_says_where_it_ends_and_creates_nothing(self):
        song = FakeArrangementSong(); song.song_length = 256.0
        cls = type("EndedArrangementSong", (type(song),), {"current_song_time": property(lambda self: self.__dict__.get("_time", 0.0), lambda self, value: (_ for _ in ()).throw(RuntimeError("Invalid position")) if float(value) > 256.0 else self.__dict__.__setitem__("_time", float(value)))})
        song.__class__ = cls; mapper = LiveObjectMapper(song)
        before = [item["name"] for item in mapper.discover("locator")["items"]]
        create_args = {"name": "Far", "position": 4096, "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"]}
        with self.assertRaisesRegex(ValueError, "past the end of the Set: its arrangement ends at beat 256"): mapper.invoke("arrangement.locator.create", create_args)
        self.assertEqual([item["name"] for item in mapper.discover("locator")["items"]], before)


class ArgumentError(TypeError):
    """Boost.Python's error when a call's Python arguments don't match Live's C++ signature."""


def float32(value):
    return struct.unpack("f", struct.pack("f", float(value)))[0]


class FakeMidiNote:
    """Live 12's MidiNote: attributes, with its values kept as 32-bit floats."""
    FLOATS = ("start_time", "duration", "velocity", "probability", "velocity_deviation", "release_velocity")

    def __init__(self, note_id, pitch, start_time, duration, velocity=100.0, mute=False, probability=1.0, velocity_deviation=0.0, release_velocity=64.0):
        self.note_id = note_id; self.pitch = pitch; self.start_time = start_time; self.duration = duration; self.velocity = velocity
        self.mute = mute; self.probability = probability; self.velocity_deviation = velocity_deviation; self.release_velocity = release_velocity

    def __setattr__(self, name, value):
        if name == "pitch" and not isinstance(value, int): raise ArgumentError("pitch takes an int")
        object.__setattr__(self, name, float32(value) if name in self.FLOATS else value)

    def copy(self):
        return FakeMidiNote(self.note_id, self.pitch, self.start_time, self.duration, self.velocity, self.mute, self.probability, self.velocity_deviation, self.release_velocity)


class FakeMidiNoteVector:
    """Live's MidiNoteVector: iterating gives its own notes, which edits change in place."""

    def __init__(self, notes): self._notes = list(notes)
    def __iter__(self): return iter(self._notes)
    def __len__(self): return len(self._notes)


class FakeNoteClip(FakeClip):
    """A MIDI clip with Live 12's note API: reads hand out copies of the notes in a MidiNoteVector, and
    apply_note_modifications takes exactly such a vector back (a Python list is an ArgumentError)."""

    def __init__(self, length=4.0, notes=()):
        super().__init__(length); self.is_audio_clip = False; self.stored = {note.note_id: note for note in notes}; self.refuse = False

    def get_all_notes_extended(self): return FakeMidiNoteVector(note.copy() for note in sorted(self.stored.values(), key=lambda note: note.note_id))
    def get_notes_extended(self, *_): return self.get_all_notes_extended()
    def get_notes_by_id(self, ids): return FakeMidiNoteVector(self.stored[note_id].copy() for note_id in ids if note_id in self.stored)

    def apply_note_modifications(self, notes):
        if not isinstance(notes, FakeMidiNoteVector): raise ArgumentError("Python argument types in Clip.apply_note_modifications(Clip, list) did not match C++ signature")
        if self.refuse: raise RuntimeError("Live refused the modification")
        for note in notes:
            if note.note_id in self.stored: self.stored[note.note_id] = note.copy()


class NoteModificationTests(unittest.TestCase):
    def clip_mapper(self, clip):
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = clip; mapper = LiveObjectMapper(song, provenance="real-live")
        row = mapper.snapshot()["tracks"][0]["clips"][0]
        return mapper, row["ref"]

    def update(self, mapper, reference, notes):
        request = {"ref": reference, "notes": notes, "expectedClipAuthority": mapper._session_clip_authority(reference), "expectedNotesRevision": hashlib.sha256(mapper._bounded_canonical(mapper._read_notes(mapper.refs.get(reference))).encode()).hexdigest()}
        validate_operation_payload("note.update", "request", request)
        result = mapper.invoke("note.update", request); validate_operation_payload("note.update", "result", result)
        return result

    def test_note_update_hands_lives_own_note_vector_back(self):
        clip = FakeNoteClip(4.0, [FakeMidiNote(1, 60, 0.0, 1.0, 100.0), FakeMidiNote(2, 64, 1.0, 1.0, 80.0, probability=0.5)])
        mapper, reference = self.clip_mapper(clip)
        self.assertEqual(self.update(mapper, reference, [{"id": 1, "velocity": 90}]), {"updated": 1})
        self.assertEqual((clip.stored[1].velocity, clip.stored[2].velocity), (90.0, 80.0))
        # A transpose in place moves every note's pitch; a 32-bit float field (0.7) is confirmed within its precision.
        self.update(mapper, reference, [{"id": 1, "pitch": 62}, {"id": 2, "pitch": 66, "probability": 0.7}])
        self.assertEqual((clip.stored[1].pitch, clip.stored[2].pitch), (62, 66)); self.assertNotEqual(clip.stored[2].probability, 0.7); self.assertAlmostEqual(clip.stored[2].probability, 0.7, places=6)

    def quantize_fixture(self, swing=0.0):
        class RecordingQuantization(int): pass  # Live's enum members are int subclasses
        members = {name: RecordingQuantization(number) for number, name in enumerate(("rec_q_no_q", "rec_q_quarter", "rec_q_eight", "rec_q_eight_triplet", "rec_q_eight_eight_triplet", "rec_q_sixtenth", "rec_q_sixtenth_triplet", "rec_q_sixtenth_sixtenth_triplet", "rec_q_thirtysecond"))}
        live = types.SimpleNamespace(Song=types.SimpleNamespace(RecordingQuantization=types.SimpleNamespace(**members)))
        clip = FakeNoteClip(4.0, [FakeMidiNote(1, 60, 0.1, 0.5), FakeMidiNote(2, 64, 0.3, 0.5)]); mapper, reference = self.clip_mapper(clip); mapper.song.swing_amount = swing; grids = []
        def quantize(grid, amount):
            grids.append(grid)
            if not isinstance(grid, RecordingQuantization): raise ArgumentError("Clip.quantize takes a RecordingQuantization")
            step = {1: 1.0, 2: 0.5, 3: 1.0 / 3.0, 5: 0.25, 6: 1.0 / 6.0, 8: 0.125}[int(grid)]
            for note in clip.stored.values():
                index = round(note.start_time / step); note.start_time = index * step + (step * swing / 2 if index % 2 else 0.0)  # Live swings every other step
        clip.quantize = quantize
        def request(grid):
            return {"ref": reference, "grid": grid, "amount": 1.0, "expectedClipAuthority": mapper._session_clip_authority(reference), "expectedNotesRevision": hashlib.sha256(mapper._bounded_canonical(mapper._read_notes(clip)).encode()).hexdigest()}
        return live, members, clip, mapper, request, grids

    def test_quantize_passes_lives_recording_quantization_for_the_grid(self):
        live, members, clip, mapper, request, grids = self.quantize_fixture()
        with patch.dict(sys.modules, {"Live": live}):
            validate_operation_payload("note.quantize", "request", request(0.25))
            self.assertTrue(mapper.invoke("note.quantize", request(0.25))["changed"])
            self.assertIs(grids[-1], members["rec_q_sixtenth"])
            self.assertTrue(mapper.invoke("note.quantize", request(0.3333))["changed"]); self.assertIs(grids[-1], members["rec_q_eight_triplet"])
            with self.assertRaisesRegex(ValueError, "supported quantization grid"): mapper.invoke("note.quantize", request(2.0))
        self.assertEqual([note.start_time for note in clip.stored.values()], [0.0, float32(1.0 / 3.0)])

    def test_quantize_with_the_sets_swing_is_confirmed_and_a_wrong_result_is_put_back(self):
        live, members, clip, mapper, request, grids = self.quantize_fixture(swing=0.2)
        with patch.dict(sys.modules, {"Live": live}):
            self.assertTrue(mapper.invoke("note.quantize", request(0.25))["changed"])
        self.assertEqual([note.start_time for note in clip.stored.values()], [0.0, float32(0.275)])
        # A result quantizing doesn't produce (here a pitch changed) is refused, and the notes go back exactly.
        before = [(note.pitch, note.start_time) for note in clip.stored.values()]
        def wrong(grid, amount):
            for note in clip.stored.values(): note.start_time = 0.5; note.pitch = 70
        clip.quantize = wrong
        with patch.dict(sys.modules, {"Live": live}), self.assertRaisesRegex(ValueError, "^quantization was not confirmed$"): mapper.invoke("note.quantize", request(0.25))
        self.assertEqual([(note.pitch, note.start_time) for note in clip.stored.values()], before)

    def test_a_note_update_live_refuses_reports_that_error_not_a_failed_rollback(self):
        clip = FakeNoteClip(4.0, [FakeMidiNote(1, 60, 0.0, 1.0)]); clip.refuse = True
        mapper, reference = self.clip_mapper(clip)
        with self.assertRaisesRegex(RuntimeError, "Live refused the modification"): self.update(mapper, reference, [{"id": 1, "velocity": 90}])
        self.assertEqual(clip.stored[1].velocity, 100.0)


def defer_writes(obj, *names):
    """Make obj apply writes to names on Live's next tick (obj.tick()), as Live 12.4 does for the
    transport and a track's arm: reading right after the write still gives the old value."""
    values = {name: getattr(obj, name) for name in names}; pending = []
    properties = {name: property(lambda self, name=name: values[name], lambda self, value, name=name: pending.append((name, value))) for name in names}
    obj.__class__ = type(f"Deferred{type(obj).__name__}", (type(obj),), properties)
    def tick():
        for name, value in pending: values[name] = value
        pending.clear()
    obj.tick = tick
    return obj


class DeferredLiveWriteTests(unittest.TestCase):
    """A write Live applies on its next tick reads as before right after it: that is pending, not a
    failure (the host confirms it in fresh state). Any other readback still refuses and rolls back."""

    def transport_song(self):
        song = FakeSong(); song.loop = False; song.loop_start = 8.0; song.loop_length = 16.0; song.metronome = False; song.punch_in = False; song.punch_out = False; song.current_song_time = 4096.0; song.count_in_duration = 0
        return song

    def transport_request(self, mapper, **fields):
        snapshot = mapper.snapshot()
        return {"setRef": snapshot["set"]["ref"], "expectedObjectIdentity": snapshot["set"]["objectIdentity"], "expectedRevision": snapshot["playback"]["revision"], **fields}

    def routing_request(self, mapper, **fields):
        row = mapper.snapshot()["tracks"][0]; routing = row["routing"]
        state = {"inputType": routing["inputType"], "inputSubRouting": routing["inputSubRouting"], "outputType": routing["outputType"], "outputSubRouting": routing["outputSubRouting"], "arm": row["armed"], "monitoring": row["monitoringState"]}
        return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest(), **fields}

    def test_loop_and_metronome_that_live_applies_on_its_next_tick_are_accepted(self):
        song = defer_writes(self.transport_song(), "loop", "loop_start", "loop_length", "metronome", "current_song_time"); mapper = LiveObjectMapper(song)
        request = self.transport_request(mapper, loopEnabled=True, loopStart=16.0, loopLength=16.0, metronome=True)
        validate_operation_payload("transport.set", "request", request)
        result = mapper.invoke("transport.set", request); validate_operation_payload("transport.set", "result", result)
        self.assertEqual((song.loop, song.loop_start, song.loop_length, song.metronome), (False, 8.0, 16.0, False), "nothing applied before Live's tick")
        song.tick()
        transport = mapper.snapshot()["playback"]["transport"]
        self.assertEqual((transport["loop"], transport["metronome"]), ({"enabled": True, "start": 16.0, "length": 16.0}, True))

    def test_a_transport_value_live_changed_to_something_else_still_refuses_and_rolls_back(self):
        class ClampingSong(FakeSong):
            @property
            def loop_start(self): return self.__dict__.get("_loop_start", 0.0)
            @loop_start.setter
            def loop_start(self, value): self.__dict__["_loop_start"] = min(float(value), 12.0)
        song = ClampingSong(); song.loop = True; song.loop_start = 8.0; song.loop_length = 16.0; song.metronome = False; song.punch_in = False; song.punch_out = False; song.current_song_time = 0.0
        mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(ValueError, "transport change was not confirmed by fresh state"): mapper.invoke("transport.set", self.transport_request(mapper, loopStart=16.0, metronome=True))
        self.assertEqual((song.loop_start, song.metronome), (8.0, False))

    def test_a_loop_or_playhead_past_the_end_of_the_set_says_where_it_ends_and_changes_nothing(self):
        class EndedSong(FakeSong):
            song_length = 1536.0
            def _guard(self, name, value):
                if float(value) > self.song_length: raise RuntimeError("Invalid position")
                self.__dict__[name] = float(value)
            loop_start = property(lambda self: self.__dict__.get("_loop_start", 0.0), lambda self, value: self._guard("_loop_start", value))
            current_song_time = property(lambda self: self.__dict__.get("_time", 0.0), lambda self, value: self._guard("_time", value))
        song = EndedSong(); song.loop = False; song.loop_start = 8.0; song.loop_length = 16.0; song.metronome = False; song.punch_in = False; song.punch_out = False; song.current_song_time = 0.0
        mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(ValueError, "past the end of the Set: its arrangement ends at beat 1536.*nothing changed"): mapper.invoke("transport.set", self.transport_request(mapper, loopEnabled=True, loopStart=4096.0, loopLength=16.0))
        self.assertEqual((song.loop, song.loop_start, song.loop_length), (False, 8.0, 16.0))
        with self.assertRaisesRegex(ValueError, "past the end of the Set"): mapper.invoke("transport.set", self.transport_request(mapper, position=4096.0))
        self.assertEqual(song.current_song_time, 0.0)
        # Another refusal from Live, inside the Set, stays as it was.
        def refuse(value): raise RuntimeError("other")
        EndedSong.loop_length = property(lambda self: 16.0, lambda self, value: refuse(value))
        with self.assertRaisesRegex(RuntimeError, "other"): mapper.invoke("transport.set", self.transport_request(mapper, loopLength=8.0))

    def test_arming_a_track_that_live_applies_on_its_next_tick_is_accepted(self):
        song = FakeSong(); track = song.tracks[0]; track.can_be_armed = True; track.arm = False; track.current_monitoring_state = 2
        defer_writes(track, "arm", "current_monitoring_state"); mapper = LiveObjectMapper(song)
        request = self.routing_request(mapper, arm=True); validate_operation_payload("routing.set", "request", request)
        self.assertTrue(mapper.invoke("routing.set", request)["changed"])
        self.assertFalse(track.arm, "not armed before Live's tick"); track.tick(); self.assertTrue(track.arm)
        request = self.routing_request(mapper, arm=False, monitoring="auto")
        self.assertTrue(mapper.invoke("routing.set", request)["changed"]); track.tick()
        self.assertEqual((track.arm, mapper.snapshot()["tracks"][0]["monitoringState"]), (False, "auto"))

    def test_a_monitoring_state_live_did_not_take_still_refuses_and_rolls_back(self):
        class StubbornTrack(FakeTrack):
            @property
            def current_monitoring_state(self): return self.__dict__.get("_monitoring", 2)
            @current_monitoring_state.setter
            def current_monitoring_state(self, value): self.__dict__["_monitoring"] = 0 if value == 1 else value
        song = FakeSong(); track = StubbornTrack(); track.can_be_armed = True; song.tracks = [track]; mapper = LiveObjectMapper(song)
        with self.assertRaisesRegex(ValueError, "routing change was not confirmed by fresh state"): mapper.invoke("routing.set", self.routing_request(mapper, arm=True, monitoring="auto"))
        self.assertEqual((track.arm, track.current_monitoring_state), (False, 2))


class MainThreadTransportTests(unittest.TestCase):
    SECRET = "0123456789abcdef0123456789abcdef"

    def setUp(self):
        import socket as _socket
        probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM); probe.bind(("127.0.0.1", 0)); self.port = probe.getsockname()[1]; probe.close()
        self.threads_before = threading.active_count()
        self.bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": self.port, "secret": self.SECRET})
        self.clients = []

    def tearDown(self):
        for client in self.clients: client.close()
        self.bridge.disconnect()

    def connect(self):
        import socket as _socket
        client = _socket.create_connection(("127.0.0.1", self.port), timeout=2); self.clients.append(client)
        return client, client.makefile("rb")

    def pump_until_readable(self, client, limit=20):
        """Tick the bridge like Live does; return how many ticks it took for data to arrive."""
        import select
        for tick in range(1, limit + 1):
            self.bridge.update_display()
            if select.select([client], [], [], 0.02)[0]: return tick
        self.fail("no data after pumping")

    def request(self, channel, sequence, method="status"):
        unsigned = channel.bound({"version": PROTOCOL, "id": f"{method}-{sequence}", "method": method, "nonce": f"{sequence:016d}", "sequence": sequence})
        return json.dumps({**unsigned, "mac": channel.sign(unsigned)}).encode("utf-8") + b"\n"

    def test_requests_are_answered_within_a_tick_on_the_main_thread(self):
        self.assertEqual(threading.active_count(), self.threads_before, "no accept or worker threads")
        client, reader = self.connect()
        self.assertLessEqual(self.pump_until_readable(client), 3)
        hello = json.loads(reader.readline())
        self.assertEqual(hello["id"], "hello")
        channel = AuthenticatedRemoteScript(self.SECRET, lambda *_: None, hello["bridgeEpoch"], hello["connectionChallenge"])
        self.assertEqual(hello["mac"], channel.sign({key: value for key, value in hello.items() if key != "mac"}))
        client.sendall(self.request(channel, 1) + self.request(channel, 2))
        self.assertLessEqual(self.pump_until_readable(client), 3)
        responses = [json.loads(reader.readline()), json.loads(reader.readline())]
        self.assertEqual([(item["id"], item["ok"]) for item in responses], [("status-1", True), ("status-2", True)])
        self.assertTrue(responses[0]["result"]["connected"])
        self.assertEqual(threading.active_count(), self.threads_before)

    def test_a_failed_authentication_closes_only_that_connection(self):
        client, reader = self.connect(); self.pump_until_readable(client); hello = json.loads(reader.readline())
        other, other_reader = self.connect(); self.pump_until_readable(other); other_hello = json.loads(other_reader.readline())
        forged = AuthenticatedRemoteScript("f" * 32, lambda *_: None, hello["bridgeEpoch"], hello["connectionChallenge"])
        client.sendall(self.request(forged, 1)); self.pump_until_readable(client)
        self.assertFalse(json.loads(reader.readline())["ok"])
        channel = AuthenticatedRemoteScript(self.SECRET, lambda *_: None, other_hello["bridgeEpoch"], other_hello["connectionChallenge"])
        other.sendall(self.request(channel, 1)); self.pump_until_readable(other)
        self.assertTrue(json.loads(other_reader.readline())["ok"])

    def test_submissions_from_the_pump_run_inline_and_other_threads_still_queue(self):
        queue = _MainThreadQueue(); ran = []
        queue.inline_thread = threading.get_ident()
        self.assertEqual(queue.submit(lambda: ran.append("inline") or "done"), "done")
        self.assertEqual(ran, ["inline"])
        worker_result = []
        worker = threading.Thread(target=lambda: worker_result.append(queue.submit(lambda: "queued")))
        worker.start()
        for _ in range(100):
            if queue.drain(): break
            time.sleep(0.01)
        worker.join(1)
        self.assertEqual(worker_result, ["queued"])

    def test_a_slow_connection_does_not_starve_the_others(self):
        # One client's reads use the whole tick's budget: the next client is still answered that tick or the next.
        client, reader = self.connect(); self.pump_until_readable(client); hello = json.loads(reader.readline())
        other, other_reader = self.connect(); self.pump_until_readable(other); other_hello = json.loads(other_reader.readline())
        busy = AuthenticatedRemoteScript(self.SECRET, lambda *_: None, hello["bridgeEpoch"], hello["connectionChallenge"])
        quiet = AuthenticatedRemoteScript(self.SECRET, lambda *_: None, other_hello["bridgeEpoch"], other_hello["connectionChallenge"])
        slow = self.bridge.mapper.status
        def status():
            time.sleep(remote_module.PUMP_BUDGET_SECONDS * 1.5); return slow()
        self.bridge.mapper.status = status
        client.sendall(b"".join(self.request(busy, sequence) for sequence in range(1, 9)))
        other.sendall(self.request(quiet, 1))
        import select
        for _ in range(2):
            self.bridge.update_display()
            if select.select([other], [], [], 0.02)[0]: break
        else: self.fail("the quiet connection waited behind the busy one")
        self.assertTrue(json.loads(other_reader.readline())["ok"])

    def test_disconnect_closes_every_connection(self):
        client, _ = self.connect(); self.pump_until_readable(client)
        self.bridge.disconnect()
        self.assertEqual((len(self.bridge._clients), len(self.bridge._connections)), (0, 0))
        self.bridge.update_display()


class SetScaleCapTests(unittest.TestCase):
    """WS1: nothing in a Set is refused for its size. What once had a literal cap (512 notes, 256 warp
    markers, 64 take lanes, 512 scenes, 64 parameters per request...) reads and changes whole; only
    semantic bounds (MIDI's 0-127, colours) and the pump budget, met by paging, remain."""

    def test_note_operations_take_any_number_of_notes(self):
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live"); transaction = "scale-notes-transaction"
        track_ref = mapper.snapshot()["tracks"][0]["ref"]
        # A Session clip longer than 1024 beats, as the registry allows (up to 100000).
        created = mapper.invoke("clip.create", ControlSurfaceTests.clip_creation_args(mapper, track_ref, 0, kind="midi", name="Long", length=4096), transaction)
        notes = [{"pitch": 36 + index % 48, "start": index * 0.5, "duration": 0.25, "velocity": 100, "channel": 1} for index in range(2000)]
        request = {"ref": created["ref"], "notes": notes, **ControlSurfaceTests.note_authority(mapper, created["ref"])}
        validate_operation_payload("note.add-batch", "request", request)
        result = mapper.invoke("note.add-batch", request, transaction)
        self.assertEqual(result["added"], 2000); self.assertEqual(len(set(result["noteIds"])), 2000)
        clip = song.tracks[0].clip_slots[0].clip
        clip.get_notes_by_id = lambda ids: [note for note in clip.notes if note["note_id"] in set(ids)]
        read = mapper.invoke("note.read-by-id", {"ref": created["ref"], "noteIds": result["noteIds"][:1500]})
        self.assertEqual(len(read["notes"]), 1500); validate_operation_payload("note.read-by-id", "result", read)
        deleted = mapper.invoke("note.delete", {"ref": created["ref"], "noteIds": result["noteIds"][:600], **ControlSurfaceTests.note_authority(mapper, created["ref"])}, transaction)
        self.assertEqual(deleted, {"deleted": 600}); self.assertEqual(len(clip.notes), 1400)

    def test_a_note_update_patches_hundreds_of_notes_at_once(self):
        clip = FakeNoteClip(800.0, [FakeMidiNote(index, 60, float(index), 0.5) for index in range(1, 701)])
        mapper, reference = NoteModificationTests().clip_mapper(clip)
        self.assertEqual(NoteModificationTests().update(mapper, reference, [{"id": index, "velocity": 90} for index in range(1, 601)]), {"updated": 600})
        self.assertEqual(sum(1 for note in clip.stored.values() if note.velocity == 90.0), 600)

    def test_a_hundred_parameters_of_one_device_change_in_one_request(self):
        song = FakeSong(); device = song.tracks[0].devices[0]; device.parameters = [FakeParameter() for _ in range(100)]
        for index, parameter in enumerate(device.parameters): parameter.name = f"P{index}"
        mapper = LiveObjectMapper(song); rows = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"]
        authority = ControlSurfaceTests.parameter_authority(mapper, rows[0]["ref"]); shared = {key: value for key, value in authority.items() if key != "expectedObjectIdentity"}
        items = [{"ref": row["ref"], "value": 0.75, "expectedRevision": row["revision"], "expectedObjectIdentity": row["objectIdentity"]} for row in rows]
        result = mapper.invoke("device.parameters.set", {**shared, "parameters": items})
        self.assertEqual(len(result["parameters"]), 100); self.assertTrue(all(parameter.value == 0.75 for parameter in device.parameters))

    def test_collections_past_their_old_literal_caps_read_whole(self):
        class Marker:
            def __init__(self, value): self.beat_time = value; self.sample_time = value * 100.0
        song = FakeSong(); track = song.tracks[0]
        track.take_lanes = [FakeTakeLane(f"Take {index}") for index in range(70)]
        track.take_lanes[0].arrangement_clips = [FakeClip(1.0) for _ in range(300)]
        track.arrangement_clips = [FakeClip(1.0) for _ in range(300)]
        song.scenes = [FakeScene(f"Scene {index}") for index in range(600)]
        audio = FakeClip(4.0); audio.is_audio_clip = True; audio.warp_markers = [Marker(float(index)) for index in range(300)]; track.clip_slots[0].clip = audio
        song.groove_pool = FakeGroovePool([FakeGroove(f"Groove {index}") for index in range(300)]); song.groove_amount = 0.5
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot()
        row = snapshot["tracks"][0]
        self.assertEqual((len(row["takeLanes"]), len(row["takeLanes"][0]["clips"]), len(snapshot["arrangement"]["clips"]), len(snapshot["scenes"])), (70, 300, 300, 600))
        self.assertEqual(len(row["clips"][0]["warpMarkers"]), 300)
        mapper._take_lane_collection_revision(track, 0); mapper._arrangement_collection_revision(track, 0); mapper._scene_collection_revision()
        self.assertEqual(len(mapper.invoke("groove.read", {"setRef": snapshot["set"]["ref"]})["grooves"]), 300)

    def test_set_wide_reads_take_hundreds_of_tracks(self):
        song = FakeSong(); song.tracks = [FakeTrack() for _ in range(300)]
        for index, track in enumerate(song.tracks): track.name = f"Track {index}"
        song.visible_tracks = list(song.tracks)
        mapper = LiveObjectMapper(song); set_ref = mapper.snapshot()["set"]["ref"]
        self.assertEqual(len(mapper.invoke("song.read", {"setRef": set_ref})["visibleTracks"]), 300)
        performance = mapper.invoke("performance.read", {"setRef": set_ref})
        self.assertEqual(len(performance["tracks"]), 300); validate_operation_payload("performance.read", "result", performance)

    def test_an_envelope_reads_every_point(self):
        clip = FakeClip(10000.0)
        envelope = types.SimpleNamespace(canonical_parent=clip, events_in_range=lambda start, end: [types.SimpleNamespace(time=float(index), value=0.5) for index in range(700)])
        self.assertEqual(len(LiveObjectMapper(FakeSong())._envelope_points(envelope)), 700)

    def test_device_positions_go_to_the_registry_bound_but_never_past_the_siblings(self):
        song = FakeSong(); track = song.tracks[0]; track.devices = [FakeDevice() for _ in range(300)]
        def insert_device(name, index):
            device = FakeDevice(); device.name = name; track.devices.insert(len(track.devices) if index < 0 else index, device)
        track.insert_device = insert_device; track.delete_device = lambda index: track.devices.pop(index)
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        siblings = [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in row["devices"]]
        args = {"trackRef": row["ref"], "deviceName": "Utility", "expectedTrackIdentity": row["objectIdentity"], "expectedSiblings": siblings}
        with self.assertRaisesRegex(ValueError, "exceeds the exact sibling boundary"): mapper.invoke("device.insert", {**args, "index": 301})
        with self.assertRaisesRegex(ValueError, "device index is invalid"): mapper.invoke("device.insert", {**args, "index": 100001})
        self.assertEqual(mapper.invoke("device.insert", {**args, "index": 300})["index"], 300)
        moved = []
        song.move_device = lambda device, target, position: (target.devices.remove(device), target.devices.insert(position, device), moved.append(position))
        snapshot = mapper.snapshot(); row = snapshot["tracks"][0]; first = row["devices"][0]
        move_args = {"ref": first["ref"], "expectedObjectIdentity": first["objectIdentity"], "expectedOwnerRef": row["ref"], "expectedOwnerIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": device["ref"], "objectIdentity": device["objectIdentity"]} for device in row["devices"]], "expectedTrackRef": row["ref"], "expectedTrackIdentity": row["objectIdentity"]}
        self.assertEqual(mapper.invoke("device.move", {**move_args, "index": 299})["index"], 299); self.assertEqual(moved, [299])

    def test_discovery_pages_and_budgets_go_to_the_registry_bounds(self):
        mapper = LiveObjectMapper(FakeSong())
        self.assertEqual(len(mapper.discover("track", limit=100000, traversal_budget=10_000_000, requested_fields=[f"field{index}" for index in range(256)])["items"]), 1)
        for invalid in ({"limit": 100001}, {"traversal_budget": 10_000_001}, {"requested_fields": [f"field{index}" for index in range(257)]}):
            with self.assertRaises(ValueError): mapper.discover("track", **invalid)
        validate_operation_payload("discover", "request", {"kind": "track", "limit": 100000, "traversalBudget": 10_000_000})

    def test_a_browser_search_returns_as_many_results_as_the_registry_allows(self):
        folder = types.SimpleNamespace(name="Synths", is_loadable=False, is_device=False, children=[types.SimpleNamespace(name=f"Preset {index}", is_loadable=True, is_device=False, children=[]) for index in range(3000)])
        browser = types.SimpleNamespace(instruments=types.SimpleNamespace(name="instruments", children=[folder]))
        mapper = LiveObjectMapper(FakeSong()); mapper._browser = lambda: browser
        request = {"category": "instruments", "query": "preset", "limit": 2500}
        validate_operation_payload("browser.search", "request", request)
        self.assertEqual(len(mapper.invoke("browser.search", request)["items"]), 2500)
        with self.assertRaisesRegex(ValueError, "browser limit is invalid"): mapper.invoke("browser.search", {**request, "limit": 10001})

    def test_a_request_may_carry_sixty_four_arguments(self):
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: {"lanes": []})
        def invoke(sequence, count):
            unsigned = remote.bound({"version": PROTOCOL, "id": f"args-{sequence}", "method": "invoke", "operation": "audio.take-lane.read", "args": {f"a{index}": index for index in range(count)}, "nonce": f"args-nonce-{sequence:08d}", "sequence": sequence})
            return remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertNotEqual(invoke(1, 64)["error"], "args must be a bounded object", "64 arguments reach the registry's own check")
        self.assertEqual(invoke(2, 65)["error"], "args must be a bounded object")

    def test_ledgers_hold_thousands_of_authorities_and_owned_objects(self):
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(FakeSong(), provenance="real-live"); bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
        class ImmediateQueue:
            def submit(self, action, deadline_ms=None, on_cancel=None): return action()
        bridge.queue = ImmediateQueue(); holder = {}
        parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        request = {"operation": "device.parameter.set", "transactionId": "transaction-ledgers", "args": {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(bridge.mapper, parameter["ref"])}}
        preflights = [bridge._dispatch_with_holder("preflight", request, holder) for _ in range(100)]
        self.assertEqual(len(holder["preflights"]), 100)
        for index in range(300): bridge._executed_mutations[f"filler-{index:04d}"] = {"operation": "track.set", "argsDigest": "0" * 64, "transactionId": "filler", "result": {}}
        prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflights[-1]["preflightToken"], "confirmation": preflights[-1]["confirmation"], "idempotencyKey": "ledger-apply"}, holder)
        self.assertEqual(bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder)["value"], 0.75)
        mapper = bridge.mapper
        for index in range(5000): mapper._owned_cleanup_tokens[f"filler-token-{index}"] = {"transactionId": "filler", "ref": f"{mapper.refs.epoch + 1}:track:{index}", "objectIdentity": f"filler:{index}", "fingerprint": "0" * 64}
        created = mapper.invoke("track.create", {"name": "Owned past 4096", "kind": "midi", "index": 1, "expectedStructureRevision": mapper._structure_revision()}, "transaction-ledgers")
        self.assertIn("ownershipToken", created)


class _BridgeSocketFixture:
    """A bridge on a real loopback socket, ticked by hand as Live's display does. Lines are read from
    the raw socket while ticking: the bridge only writes during a tick."""
    SECRET = "0123456789abcdef0123456789abcdef"

    def setUp(self):
        import socket as _socket
        probe = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM); probe.bind(("127.0.0.1", 0)); self.port = probe.getsockname()[1]; probe.close()
        self.bridge = AbletonMcpBridge(FakeInstance(), {"host": "127.0.0.1", "port": self.port, "secret": self.SECRET})
        self.clients = []; self.buffers = {}

    def tearDown(self):
        for client in self.clients: client.close()
        self.bridge.disconnect()

    def connect(self):
        import socket as _socket
        client = _socket.create_connection(("127.0.0.1", self.port), timeout=5); self.clients.append(client); self.buffers[client] = bytearray()
        hello = self.read_lines(client, 1)[0]
        channel = AuthenticatedRemoteScript(self.SECRET, lambda *_: None, hello["bridgeEpoch"], hello["connectionChallenge"])
        return client, channel

    def read_lines(self, client, count, seconds=10.0):
        """Tick the bridge until count response lines arrived; return them parsed. Bounded by time, not
        ticks: on a busy machine the loopback lags behind ticks that wait for nothing."""
        import select
        buffer = self.buffers[client]; lines = []
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.bridge.update_display()
            while select.select([client], [], [], 0.001)[0]:
                chunk = client.recv(1 << 20)
                if not chunk: break
                buffer.extend(chunk)
            while len(lines) < count:
                index = buffer.find(b"\n")
                if index < 0: break
                lines.append(json.loads(bytes(buffer[:index]))); del buffer[:index + 1]
            if len(lines) >= count: return lines
        self.fail(f"only {len(lines)} of {count} lines after ticking")

    def frame(self, channel, sequence, method="status", **fields):
        unsigned = channel.bound({"version": PROTOCOL, "id": f"{method}-{sequence}", "method": method, "nonce": f"{sequence:016d}", "sequence": sequence, **fields})
        return json.dumps({**unsigned, "mac": channel.sign(unsigned)}).encode("utf-8") + b"\n"


class RegistryLoadTests(unittest.TestCase):
    def test_the_registry_is_read_and_checked_once_not_per_request(self):
        saved = remote_module._REGISTRY_CACHE
        loads = []
        original = remote_module._load_operation_registry
        def counting():
            loads.append(True); return original()
        try:
            remote_module._REGISTRY_CACHE = None
            with patch.object(remote_module, "_load_operation_registry", counting):
                for _ in range(50):
                    validate_operation_payload("status", "request", {})
                    validate_operation_payload("snapshot", "request", {"focus": [1]})
                registry, digest = operation_registry()
            self.assertEqual(len(loads), 1)
            self.assertEqual(digest, hashlib.sha256(json.dumps(registry, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest())
            with self.assertRaisesRegex(ValueError, "not in canonical registry"): validate_operation_payload("no.such-operation", "request", {})
        finally:
            remote_module._REGISTRY_CACHE = saved


class LingeringTickTests(_BridgeSocketFixture, unittest.TestCase):
    """A change's steps (read, change, confirm) each wait for the answer before them. A tick that answered
    someone waits a few milliseconds for the next step, so the steps don't each cost a display tick."""

    def test_requests_sent_right_after_their_answers_are_served_in_the_same_tick(self):
        client, channel = self.connect(); answered = []
        def ask():
            buffer = bytearray()
            for sequence in (1, 2, 3):
                client.sendall(self.frame(channel, sequence))
                while b"\n" not in buffer:
                    chunk = client.recv(1 << 20)
                    if not chunk: return
                    buffer.extend(chunk)
                index = buffer.find(b"\n"); answered.append(json.loads(bytes(buffer[:index]))); del buffer[:index + 1]
        asker = threading.Thread(target=ask); asker.start()
        deadline = time.time() + 2
        while not answered and time.time() < deadline: self.bridge.update_display(); time.sleep(0.001)
        ticks_after_first = 0
        while len(answered) < 3 and time.time() < deadline: self.bridge.update_display(); ticks_after_first += 1
        asker.join(timeout=2)
        self.assertEqual([item["id"] for item in answered], ["status-1", "status-2", "status-3"])
        self.assertEqual(ticks_after_first, 0, "the two later steps were served in the tick that answered the first")

    def test_a_read_the_tick_takes_late_ends_with_the_tick(self):
        # A paged read served while the tick waited for the next request mustn't stretch the tick past its budget.
        mapper = LiveObjectMapper(FakeSong()); mapper.read_budget_seconds = 10
        with patch.object(remote_module.time, "perf_counter", return_value=1000.0):
            self.assertEqual(mapper._read_budget().deadline, 1010.0, "outside a tick, a read has its own budget")
            mapper.tick_deadline = 1000.005
            self.assertEqual(mapper._read_budget().deadline, 1000.005, "inside one, no later than the tick's end")
            mapper.tick_deadline = 999.0
            budget = mapper._read_budget(); self.assertEqual([budget.room(), budget.room()], [True, False], "past it, one unit")

    @unittest.skipUnless(os.path.isdir("/dev/fd"), "counts open descriptors in /dev/fd")
    def test_lingering_ticks_leave_no_descriptor_open(self):
        # Each lingering tick makes a selector (a kqueue on macOS); a leak would run Live out of descriptors.
        client, channel = self.connect(); buffer = bytearray()
        def answer_once(sequence):
            client.sendall(self.frame(channel, sequence))
            while b"\n" not in buffer:
                self.bridge.update_display()
                while select_module.select([client], [], [], 0)[0]: buffer.extend(client.recv(1 << 20))
            del buffer[:buffer.find(b"\n") + 1]
        answer_once(1); before = len(os.listdir("/dev/fd"))
        for sequence in range(2, 152): answer_once(sequence)
        self.assertLessEqual(len(os.listdir("/dev/fd")), before)

    def answered_between_ticks(self, client, seconds=2.0):
        """Serve the bridge from its timer only (no display tick) until an answer arrives, or time's up."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.bridge.serve_between_ticks()
            if select_module.select([client], [], [], 0.001)[0]: return client.recv(1 << 20)
        return b""

    def test_between_ticks_a_change_is_served_without_a_display_tick(self):
        # Its first request when the timer next fires; the ones after it (read, change, confirm) at once,
        # as a tick serves them, in the same turn.
        client, channel = self.connect(); self.bridge.between_ticks = True; answered = []
        def ask():
            buffer = bytearray()
            for sequence in (1, 2, 3):
                client.sendall(self.frame(channel, sequence))
                while b"\n" not in buffer:
                    chunk = client.recv(1 << 20)
                    if not chunk: return
                    buffer.extend(chunk)
                index = buffer.find(b"\n"); answered.append(json.loads(bytes(buffer[:index]))); del buffer[:index + 1]
        asker = threading.Thread(target=ask); asker.start()
        deadline = time.time() + 2; turns = 0
        while len(answered) < 3 and time.time() < deadline: self.bridge.serve_between_ticks(); turns += 1; time.sleep(0.001)
        asker.join(timeout=2)
        self.assertEqual([item["id"] for item in answered], ["status-1", "status-2", "status-3"])
        started = time.perf_counter()
        for _ in range(50): self.bridge.serve_between_ticks()
        self.assertLess((time.perf_counter() - started) / 50, remote_module.PUMP_LINGER_SECONDS / 4, "nothing waiting: back at once")

    def frozen_window(self, spent=0.0):
        """A budget window that doesn't roll over while the test runs (on a slow runner 100 ms pass in no
        time), with spent of it used."""
        self.bridge._window_started = time.perf_counter() + 60; self.bridge._window_spent = spent

    def test_the_timer_and_the_ticks_share_one_budget_a_tick(self):
        client, channel = self.connect(); self.bridge.between_ticks = True
        self.frozen_window(remote_module.PUMP_BUDGET_SECONDS)
        client.sendall(self.frame(channel, 1)); time.sleep(0.05)
        for _ in range(5): self.bridge.serve_between_ticks()
        self.bridge.update_display()
        self.assertFalse(select_module.select([client], [], [], 0.05)[0], "this tick's share is spent: no service until the next")
        self.frozen_window()
        self.assertIn(b'"status-1"', self.answered_between_ticks(client), "the next tick's share serves it")
        self.assertGreater(self.bridge._window_spent, 0.0, "and counts toward it")
        # A window rolls over by itself once a tick's worth of time has passed.
        self.bridge._window_started = time.perf_counter() - remote_module.PUMP_WINDOW_SECONDS; self.bridge._window_spent = remote_module.PUMP_BUDGET_SECONDS
        self.assertEqual(self.bridge._budget_left(), remote_module.PUMP_BUDGET_SECONDS)

    def test_a_change_takes_what_live_takes_and_reads_keep_to_the_budget(self):
        client, channel = self.connect(); self.bridge.between_ticks = True
        auth = self.bridge._connections[0].auth; answer = auth.dispatch
        def slow(request):
            # Long enough that a change counted shows over lingering (Windows rounds a 12 ms wait up to 15.6 ms).
            time.sleep(0.1); return answer(request)
        auth.dispatch = slow
        self.frozen_window()
        client.sendall(self.frame(channel, 1, method="mutate")); self.answered_between_ticks(client)
        self.assertLess(self.bridge._window_spent, 0.05, "a change's own time isn't counted")
        client.sendall(self.frame(channel, 2)); self.answered_between_ticks(client)
        self.assertGreaterEqual(self.bridge._window_spent, 0.1, "a read's is")
        self.frozen_window()
        client.sendall(self.frame(channel, 3, method="invoke", operation="browser.search", args={})); self.answered_between_ticks(client)
        self.assertGreaterEqual(self.bridge._window_spent, 0.1, "a read made through invoke counts")
        self.frozen_window()
        client.sendall(self.frame(channel, 4, method="invoke", operation="python.run", args={})); self.answered_between_ticks(client)
        self.assertGreaterEqual(self.bridge._window_spent, 0.1, "so does Python")
        self.frozen_window()
        client.sendall(self.frame(channel, 5, method="invoke", operation="track.create", args={})); self.answered_between_ticks(client)
        self.assertLess(self.bridge._window_spent, 0.05, "a change made through invoke doesn't")

    def test_a_tick_that_answered_no_one_doesnt_wait(self):
        self.connect()
        started = time.perf_counter()
        for _ in range(20): self.bridge.update_display()
        self.assertLess((time.perf_counter() - started) / 20, remote_module.PUMP_LINGER_SECONDS / 2)


class LargeFrameTransportTests(_BridgeSocketFixture, unittest.TestCase):
    def test_a_big_frame_arriving_in_pieces_is_scanned_once_and_answered(self):
        client, channel = self.connect()
        connection = self.bridge._connections[0]
        body = b"x" * (8 * 1024 * 1024)
        for offset in range(0, len(body), 256 * 1024):
            client.sendall(body[offset:offset + 256 * 1024])
            self.bridge.update_display()
        deadline = time.time() + 5
        while len(connection.inbound) < len(body) and time.time() < deadline: self.bridge.update_display()
        # Everything that came is searched once: the next piece is searched from where this one ended.
        self.assertEqual(len(connection.inbound), len(body)); self.assertEqual(connection.scanned, len(body))
        client.sendall(b"\n" + self.frame(channel, 1))
        malformed, answered = self.read_lines(client, 2)
        self.assertEqual(malformed["error"], "malformed request"); self.assertTrue(answered["ok"])
        self.assertEqual((len(connection.inbound), connection.scanned), (0, 0))

    def test_many_pipelined_frames_are_all_answered_in_order(self):
        client, channel = self.connect()
        client.sendall(b"".join(self.frame(channel, sequence) for sequence in range(1, 201)))
        answered = self.read_lines(client, 200)
        self.assertEqual([response["id"] for response in answered], [f"status-{sequence}" for sequence in range(1, 201)])
        self.assertTrue(all(response["ok"] for response in answered))

    def test_a_big_response_leaves_in_pieces_and_the_buffer_empties(self):
        song = self.bridge.mapper.song; song.tracks = [FakeTrack() for _ in range(300)]
        for track in song.tracks: track.devices = [FakeDevice() for _ in range(8)]
        client, channel = self.connect()
        client.sendall(self.frame(channel, 1, method="snapshot"))
        response = self.read_lines(client, 1)[0]
        self.assertTrue(response["ok"]); self.assertEqual(len(response["result"]["tracks"]), 300)
        connection = self.bridge._connections[0]
        self.assertEqual((connection.pending_outbound(), len(connection.outbound), connection.sent), (0, 0, 0))


def rich_song(links=True, playing=True, tracks=6):
    """A Set with something of everything: MIDI and audio clips, a group, a rack with a nested device,
    a Drum Rack, take lanes, Arrangement clips, a return, the main track, locators and a selection
    three levels deep. With links, Live's canonical_parent chain from each object up to its track."""
    song = FakeSong()
    song.name = "Rich"; song.tempo = 124.0; song.loop = True; song.loop_length = 16.0; song.loop_start = 0.0
    song.cue_points = [FakeLocator(0, "Intro"), FakeLocator(16, "Drop")]; song.set_or_delete_cue = lambda: None
    rows = []
    for index in range(tracks):
        track = FakeTrack(); track.name = f"T{index}"
        track.clip_slots = [FakeSlot() for _ in range(4)]
        track.mixer_device = FakeMixerDevice(); track.mute = index == 2; track.solo = False
        track.color_index = index % 70; track.color = 0x112233 + index
        track.arrangement_clips = [FakeClip(2.0) for _ in range(index % 3)]
        for clip in track.arrangement_clips: clip.start_time = 4.0
        rows.append(track)
    beat = FakeClip(8.0); beat.name = "Beat"; beat.add_new_notes([{"pitch": 36, "start_time": 0.0, "duration": 0.25, "velocity": 100}, {"pitch": 38, "start_time": 1.0, "duration": 0.25, "velocity": 90}])
    rows[0].clip_slots[1].clip = beat
    if playing: rows[0].playing_slot_index = 1; rows[3].fired_slot_index = 2
    rows[3].clip_slots[2].clip = FakeClip(4.0)
    class Marker:
        def __init__(self, value): self.beat_time = value; self.sample_time = value * 100.0
    audio = FakeClip(4.0); audio.is_audio_clip = True; audio.warp_markers = [Marker(0.0), Marker(2.0)]; audio.gain = 0.5; audio.file_path = "/tmp/a.wav"
    rows[1].has_midi_input = False; rows[1].clip_slots[0].clip = audio
    rows[2].is_foldable = True; rows[3].group_track = rows[2]
    rack = FakeDevice(); rack.name = "Rack"; rack.can_have_chains = True
    nested = FakeDevice(); nested.name = "Nested"
    chain = type("Chain", (), {})(); chain.name = "C1"; chain.devices = [nested]; chain.mute = False; chain.solo = False
    rack.chains = [chain]; rack.macros = [FakeParameter()]
    rows[4].devices = [rack, FakeDevice()]
    drum = FakeDevice(); drum.name = "Drums"; drum.can_have_chains = True; drum.can_have_drum_pads = True
    kick = FakeDevice(); kick.name = "Kick"
    pad_chain = type("Chain", (), {})(); pad_chain.name = "Kick"; pad_chain.devices = [kick]; pad_chain.mute = False; pad_chain.solo = False; pad_chain.in_note = 36
    drum.chains = [pad_chain]; drum.macros = []
    pad = type("Pad", (), {})(); pad.name = "Kick"; pad.note = 36; pad.chains = [pad_chain]; pad.mute = False; pad.solo = False
    drum.drum_pads = [pad]; rows[5].devices = [drum]
    lane = FakeTakeLane("Take A"); lane.arrangement_clips = [FakeClip(1.0)]; rows[0].take_lanes = [lane]
    returned = FakeTrack(); returned.name = "A-Reverb"; returned.mixer_device = FakeMixerDevice(); returned.clip_slots = []
    main = FakeTrack(); main.name = "Main"; main.mixer_device = FakeMixerDevice(); main.clip_slots = []
    song.tracks = rows; song.return_tracks = [returned]; song.master_track = main
    song.scenes = [FakeScene(f"S{index}") for index in range(4)]
    rows[4].view = type("TrackView", (), {"selected_device": rack})()
    song.view = type("View", (), {"selected_track": rows[4], "selected_scene": song.scenes[1], "highlighted_clip_slot": rows[4].clip_slots[1], "detail_clip": beat, "selected_parameter": nested.parameters[0], "selected_chain": chain})()
    if links:
        for track in rows + [returned, main]:
            for slot in track.clip_slots:
                slot.canonical_parent = track
                if slot.clip is not None: slot.clip.canonical_parent = slot
            for device in track.devices: device.canonical_parent = track
            track.mixer_device.canonical_parent = track
        nested.canonical_parent = chain; chain.canonical_parent = rack; kick.canonical_parent = pad_chain; pad_chain.canonical_parent = drum
        for device in (rack, nested, drum, kick) + tuple(rows[4].devices):
            for parameter in device.parameters: parameter.canonical_parent = device
    return song


class ReadCounter:
    """Counts reads of the attributes below a track, per track: a light row must read none of them."""
    BELOW = ("clip_slots", "devices", "take_lanes", "mixer_device", "arrangement_clips", "input_routing_type", "output_routing_type", "available_input_routing_types", "available_output_routing_types", "view")

    def __init__(self, tracks):
        self.reads = {}
        for index, track in enumerate(tracks):
            counter = self
            class Counted(type(track)):
                def __getattribute__(self, name, index=index):
                    if name in ReadCounter.BELOW: counter.reads.setdefault(index, set()).add(name)
                    return object.__getattribute__(self, name)
            track.__class__ = Counted


def canonical(value):
    return json.dumps(value, sort_keys=True)


class FocusedSnapshotTests(unittest.TestCase):
    """WS1/WS2: a snapshot takes windows, a focus and parts, and builds only what they name; the
    rows it builds whole are the whole Set's rows, byte for byte."""

    def test_without_arguments_it_is_the_whole_set_with_its_counts(self):
        mapper = LiveObjectMapper(rich_song()); snapshot = mapper.snapshot()
        self.assertEqual(list(snapshot), ["set", "tracks", "scenes", "arrangement", "playback", "selection", "epoch", "trackCount", "sceneCount"])
        self.assertEqual((snapshot["trackCount"], snapshot["sceneCount"], len(snapshot["tracks"]), len(snapshot["scenes"])), (8, 4, 8, 4))
        self.assertNotIn("window", snapshot, "no arguments, no window: the host reads that as the whole Set")
        self.assertFalse(any(row.get("light") for row in snapshot["tracks"]))
        validate_operation_payload("snapshot", "result", snapshot)
        self.assertEqual(canonical(mapper.snapshot({})), canonical({**snapshot, "playback": mapper.snapshot({})["playback"]}))

    def test_windows_page_whole_rows_and_echo_what_they_asked(self):
        mapper = LiveObjectMapper(rich_song()); whole = mapper.snapshot()
        page = mapper.snapshot({"tracks": {"from": 1, "count": 3}, "scenes": {"from": 2, "count": 5}})
        validate_operation_payload("snapshot", "result", page)
        self.assertEqual([row["ref"] for row in page["tracks"]], [row["ref"] for row in whole["tracks"][1:4]])
        self.assertEqual([canonical(row) for row in page["tracks"]], [canonical(row) for row in whole["tracks"][1:4]], "the same rows, byte for byte")
        self.assertEqual([row["ref"] for row in page["scenes"]], [row["ref"] for row in whole["scenes"][2:]])
        self.assertEqual((page["trackCount"], page["sceneCount"]), (8, 4))
        self.assertEqual(page["window"], {"tracks": {"from": 1, "count": 3}, "scenes": {"from": 2, "count": 5}})
        # A window's Arrangement clips are its tracks' (tracks 1-3 hold 1, 2 and 0 clips).
        self.assertEqual({clip["trackRef"] for clip in page["arrangement"]["clips"]}, {whole["tracks"][1]["ref"], whole["tracks"][2]["ref"]})
        # Past the end, a window is empty, not refused.
        beyond = mapper.snapshot({"tracks": {"from": 100, "count": 10}})
        self.assertEqual((beyond["tracks"], beyond["trackCount"]), ([], 8))
        # Playback and the selection are the whole Set's, however the rows are paged.
        self.assertEqual(page["playback"], whole["playback"]); self.assertEqual(page["selection"], whole["selection"])

    def test_focus_lists_every_track_the_focused_whole_and_the_rest_light(self):
        mapper = LiveObjectMapper(rich_song()); whole = mapper.snapshot()
        focused = mapper.snapshot({"focus": [4, 0]})
        validate_operation_payload("snapshot", "result", focused)
        self.assertEqual([row["ref"] for row in focused["tracks"]], [row["ref"] for row in whole["tracks"]], "every track, in order")
        for index, row in enumerate(focused["tracks"]):
            if index in (0, 4):
                self.assertEqual(canonical(row), canonical(whole["tracks"][index]), "a focused row is the whole row")
                continue
            self.assertIs(row["light"], True)
            self.assertEqual(set(row), set(LiveObjectMapper._LIGHT_TRACK_FIELDS) | {"light", "clips", "clipSlots", "devices", "takeLanes", "mixer", "routing"})
            self.assertEqual({key: row[key] for key in LiveObjectMapper._LIGHT_TRACK_FIELDS}, {key: whole["tracks"][index][key] for key in LiveObjectMapper._LIGHT_TRACK_FIELDS}, f"track {index}'s light fields are its whole row's")
            self.assertEqual((row["clips"], row["clipSlots"], row["devices"], row["takeLanes"], row["mixer"], row["routing"]), ([], [], [], [], None, None))
        self.assertEqual([clip["ref"] for clip in focused["arrangement"]["clips"]], [clip["ref"] for clip in whole["arrangement"]["clips"] if clip["trackRef"] in {whole["tracks"][0]["ref"], whole["tracks"][4]["ref"]}], "the focused tracks' Arrangement clips")
        self.assertEqual(len(focused["arrangement"]["clips"]), 1)
        focused_two = mapper.snapshot({"focus": [2]})
        self.assertEqual([clip["ref"] for clip in focused_two["arrangement"]["clips"]], [clip["ref"] for clip in whole["arrangement"]["clips"] if clip["trackRef"] == whole["tracks"][2]["ref"]])
        self.assertEqual(focused["window"], {"focus": [4, 0]})
        everything_light = mapper.snapshot({"focus": []})
        self.assertTrue(all(row["light"] for row in everything_light["tracks"])); self.assertEqual(everything_light["arrangement"]["clips"], [])
        self.assertEqual(focused["playback"], whole["playback"]); self.assertEqual(focused["selection"], whole["selection"])

    def test_light_rows_never_walk_below_their_track(self):
        song = rich_song(tracks=40, playing=False); song.view = None
        counter = ReadCounter(song.tracks)
        focused = LiveObjectMapper(song).snapshot({"focus": [7]})
        self.assertFalse(focused["tracks"][7].get("light"))
        self.assertEqual({index for index in counter.reads}, {7}, f"only the focused track is walked below: {counter.reads}")
        counter.reads.clear()
        LiveObjectMapper(song).snapshot({"focus": [], "parts": ["tracks"]})
        self.assertEqual(counter.reads, {})
        # Playback reads the slot list of a track that plays or is queued, and nothing else below a track.
        song = rich_song(tracks=40, playing=True); song.view = None; counter = ReadCounter(song.tracks)
        LiveObjectMapper(song).snapshot({"focus": []})
        self.assertEqual(counter.reads, {0: {"clip_slots"}, 3: {"clip_slots"}})

    def test_parts_build_only_what_is_asked(self):
        mapper = LiveObjectMapper(rich_song()); built = []
        original_row, original_light = LiveObjectMapper._track_row, LiveObjectMapper._light_track_row
        with patch.object(LiveObjectMapper, "_track_row", lambda self, *args: built.append("whole") or original_row(self, *args)), patch.object(LiveObjectMapper, "_light_track_row", lambda self, *args: built.append("light") or original_light(self, *args)):
            playback_only = mapper.snapshot({"parts": ["playback"]})
            self.assertEqual(built, [], "no track row for playback")
            tracks_only = mapper.snapshot({"focus": [1], "parts": ["tracks"]})
        validate_operation_payload("snapshot", "result", playback_only); validate_operation_payload("snapshot", "result", tracks_only)
        self.assertEqual(list(playback_only), ["playback", "epoch", "trackCount", "sceneCount", "window"])
        self.assertEqual(list(tracks_only), ["tracks", "epoch", "trackCount", "sceneCount", "window"])
        self.assertEqual(built, ["light", "whole"] + ["light"] * 6)
        self.assertEqual(tracks_only["window"], {"focus": [1], "parts": ["tracks"]})
        self.assertEqual(set(mapper.snapshot({"parts": []})), {"epoch", "trackCount", "sceneCount", "window"})

    def test_a_shared_read_builds_each_distinct_snapshot_once(self):
        mapper = LiveObjectMapper(rich_song()); builds = []; build = LiveObjectMapper._build_snapshot
        with patch.object(LiveObjectMapper, "_build_snapshot", lambda self, args=None: builds.append(canonical(args)) or build(self, args)):
            mapper._shared_reads(lambda: [mapper.snapshot({"focus": [1]}), mapper.snapshot({"focus": [1]}), mapper.snapshot({"focus": [2]}), mapper.snapshot(), mapper.snapshot({})])
        self.assertEqual(builds, [canonical({"focus": [1]}), canonical({"focus": [2]}), canonical({})])

    def test_playback_and_selection_read_directly_are_what_the_rows_say(self):
        for links in (True, False):
            for playing in (True, False):
                mapper = LiveObjectMapper(rich_song(links, playing)); whole = mapper.snapshot()
                self.assertEqual(mapper._playback(), whole["playback"])
                self.assertEqual(mapper._selection_row_targeted(), whole["selection"])
                self.assertTrue(all(value is not None for value in whole["selection"].values()), whole["selection"])
        song = rich_song(); mapper = LiveObjectMapper(song); song.view = None
        self.assertEqual(mapper._selection_row_targeted(), {key: None for key, _, _ in LiveObjectMapper._SELECTION_KINDS})

    def test_arguments_are_checked(self):
        mapper = LiveObjectMapper(FakeSong())
        for invalid in ({"tracks": {"from": -1, "count": 1}}, {"tracks": {"from": 0, "count": 0}}, {"scenes": {"from": 0}}, {"focus": [1, 1]}, {"focus": [True]}, {"parts": ["everything"]}, {"parts": ["set", "set"]}, {"windows": {}}):
            with self.assertRaises(ValueError): mapper.snapshot(invalid)

    def test_the_wire_carries_snapshot_arguments_both_ways(self):
        mapper = LiveObjectMapper(rich_song())
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, request: AbletonMcpBridge._dispatch_main_for(method, request, mapper))
        def ask(sequence, **fields):
            unsigned = remote.bound({"version": PROTOCOL, "id": f"snapshot-{sequence}", "method": "snapshot", "nonce": f"snapshot-nonce-{sequence:04d}", "sequence": sequence, **fields})
            return remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        focused = ask(1, args={"focus": [3], "parts": ["tracks", "playback"]})
        self.assertTrue(focused["ok"], focused); self.assertEqual(focused["result"]["window"], {"focus": [3], "parts": ["tracks", "playback"]})
        self.assertEqual([row.get("light", False) for row in focused["result"]["tracks"]], [True, True, True, False, True, True, True, True])
        plain = ask(2)
        self.assertTrue(plain["ok"]); self.assertNotIn("window", plain["result"])
        self.assertFalse(ask(3, args={"parts": ["everything"]})["ok"])


class TargetedReadTests(unittest.TestCase):
    """WS2.1: get, discover, the structure revision and mutations read what they name: the ref's own
    track, the parent's track, light rows for the Set's lists. Their answers are the snapshot's."""

    def built_tracks(self, mapper, work):
        built = []; original = LiveObjectMapper._track_row
        with patch.object(LiveObjectMapper, "_track_row", lambda self, track, kind, index: built.append(index) or original(self, track, kind, index)):
            result = work()
        return built, result

    def test_get_reads_only_the_track_a_ref_lives_on_and_answers_as_the_snapshot_does(self):
        mapper = LiveObjectMapper(rich_song(tracks=12)); whole = mapper.snapshot()
        device = whole["tracks"][4]["devices"][0]; nested = device["chains"][0]["devices"][0]
        cases = [
            # A clip from its slot, a device (and a parameter) from its track's light walk: no whole track row.
            (whole["tracks"][0]["clips"][0]["ref"], [], whole["tracks"][0]["clips"][0]),
            (nested["ref"], [], nested),
            (nested["parameters"][0]["ref"], [], nested["parameters"][0]),
            (whole["tracks"][0]["takeLanes"][0]["ref"], [0], whole["tracks"][0]["takeLanes"][0]),
            (whole["tracks"][0]["takeLanes"][0]["clips"][0]["ref"], [0], whole["tracks"][0]["takeLanes"][0]["clips"][0]),
            (whole["tracks"][7]["ref"], [7], whole["tracks"][7]),
            (whole["set"]["ref"], [], whole["set"]),
            (whole["scenes"][2]["ref"], [], whole["scenes"][2]),
            (whole["arrangement"]["locators"][1]["ref"], [], whole["arrangement"]["locators"][1]),
            (whole["arrangement"]["clips"][0]["ref"], [], whole["arrangement"]["clips"][0]),
        ]
        for reference, tracks, expected in cases:
            built, row = self.built_tracks(mapper, lambda: mapper.get(reference))
            self.assertEqual(built, tracks, reference); self.assertEqual(canonical(row), canonical(expected), reference)
        # What get never answered, it still doesn't: slots, mixer parameters, another epoch.
        for reference in (whole["tracks"][0]["clipSlots"][0]["ref"], whole["tracks"][0]["mixer"]["volumeRef"]):
            with self.assertRaisesRegex(ValueError, "unknown live ref"): mapper.get(reference)
        with self.assertRaises(KeyError): mapper.get(f"{mapper.refs.epoch + 1}:track:0")

    def test_get_of_a_moved_object_is_refused_as_before(self):
        song = rich_song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        reference = whole["tracks"][0]["clips"][0]["ref"]; song.tracks[0].clip_slots[1].clip = None
        with self.assertRaisesRegex(ValueError, "unknown live ref"): mapper.get(reference)
        track_ref = whole["tracks"][3]["ref"]; song.tracks.insert(0, FakeTrack())
        with self.assertRaisesRegex(ValueError, "unknown live ref"): mapper.get(track_ref)

    def test_discovery_under_a_parent_reads_only_the_parents_track(self):
        song = rich_song(tracks=30, playing=False); song.view = None; mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        counter = ReadCounter(song.tracks)
        device = whole["tracks"][4]["devices"][0]
        slots = mapper.discover("clip_slot", parent=whole["tracks"][4]["ref"])["items"]
        nested = mapper.discover("device", parent=device["chains"][0]["ref"])["items"]
        parameters = mapper.discover("parameter", parent=nested[0]["ref"])["items"]
        notes = mapper.discover("note", parent=whole["tracks"][0]["clips"][0]["ref"])["items"]
        self.assertEqual(set(counter.reads), {0, 4}, counter.reads)
        self.assertEqual([slot["ref"] for slot in slots], [slot["ref"] for slot in whole["tracks"][4]["clipSlots"]])
        self.assertEqual([row["ref"] for row in nested], [row["ref"] for row in device["chains"][0]["devices"]])
        self.assertEqual(len(parameters), len(device["chains"][0]["devices"][0]["parameters"])); self.assertEqual([note["pitch"] for note in notes], [36, 38])

    def test_the_sets_track_list_pages_over_light_rows_and_builds_only_its_page(self):
        song = rich_song(tracks=30, playing=False); song.view = None; mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        counter = ReadCounter(song.tracks)
        names = mapper.discover("track", limit=100, requested_fields=["name", "kind", "armed"])
        self.assertEqual(counter.reads, {}, "light fields read nothing below a track")
        self.assertEqual([item["name"] for item in names["items"]], [f"T{index}" for index in range(30)])
        built, page = self.built_tracks(mapper, lambda: mapper.discover("track", limit=2, cursor=mapper.discover("track", limit=5, requested_fields=["name"])["nextCursor"]))
        self.assertEqual(built, [5, 6], "whole rows for the page alone")
        self.assertEqual([canonical(item) for item in page["items"]], [canonical(row) for row in whole["tracks"][5:7]])
        # A filter on a field only whole rows have still works (reading them).
        muted = mapper.discover("track", filters={"performanceImpact": None}, requested_fields=["name"])
        self.assertEqual(len(muted["items"]), 30)

    def test_a_list_revision_follows_membership_names_and_order_not_values(self):
        song = rich_song(); mapper = LiveObjectMapper(song)
        first = mapper.discover("track", limit=2); cursor = first["nextCursor"]
        song.tracks[3].mixer_device.volume.value = 0.9; song.tracks[3].arm = True
        self.assertEqual(mapper.discover("track", limit=2, cursor=cursor)["revision"], first["revision"], "a value changing keeps the pages consistent")
        for change in (lambda: setattr(song.tracks[1], "name", "Renamed"), lambda: song.tracks.append(FakeTrack()), lambda: song.tracks.insert(0, song.tracks.pop(2))):
            change()
            with self.assertRaisesRegex(ValueError, "discovery cursor"): mapper.discover("track", limit=2, cursor=cursor)
            first = mapper.discover("track", limit=2); cursor = first["nextCursor"]
        scenes = mapper.discover("scene", limit=1); song.scenes[0].name = "Renamed"
        with self.assertRaisesRegex(ValueError, "discovery cursor"): mapper.discover("scene", limit=1, cursor=scenes["nextCursor"])

    def test_the_structure_revision_is_the_hosts_formula_from_light_reads(self):
        song = rich_song(tracks=20, playing=False); song.view = None; mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        host = {"tracks": [[row["ref"], row.get("objectIdentity"), row["name"], row["kind"], index] for index, row in enumerate(whole["tracks"])], "scenes": [[row["ref"], row.get("objectIdentity"), row["name"], index] for index, row in enumerate(whole["scenes"])]}
        counter = ReadCounter(song.tracks)
        self.assertEqual(mapper._structure_revision(), hashlib.sha256(json.dumps(host, ensure_ascii=False, separators=(",", ":")).encode("utf-8")).hexdigest())
        self.assertEqual(counter.reads, {})

    def test_ownership_fingerprints_read_their_object_and_keep_their_formula(self):
        song = rich_song(); mapper = LiveObjectMapper(song); whole = remote_module._expanded_pad_chains(mapper.snapshot())
        def old_track(reference):
            track = next(row for row in whole["tracks"] if row["ref"] == reference)
            owned = {**{key: value for key, value in track.items() if key not in remote_module._VOLATILE_TRACK_FIELDS}, **({"routing": {key: value for key, value in track["routing"].items() if key not in remote_module._VOLATILE_ROUTING_FIELDS}} if isinstance(track.get("routing"), dict) else {}), "clipSlots": [{key: value for key, value in slot.items() if key not in remote_module._VOLATILE_SLOT_FIELDS} for slot in track.get("clipSlots", []) if slot.get("empty") is not True or slot.get("clipRef") is not None]}
            clips = [clip for clip in whole["arrangement"]["clips"] if clip.get("trackRef") == reference or clip.get("parentRef") == reference]
            return hashlib.sha256(mapper._bounded_canonical(remote_module._without_fields({"track": owned, "arrangementClips": clips}, remote_module._VOLATILE_CLIP_FIELDS)).encode("utf-8")).hexdigest()
        def old_scene(reference):
            scene = next(row for row in whole["scenes"] if row["ref"] == reference); contents = []
            for track in whole["tracks"]:
                slot = next((row for row in track.get("clipSlots", []) if row.get("sceneIndex") == scene["index"]), None); clip = next((row for row in track.get("clips", []) if slot is not None and row.get("ref") == slot.get("clipRef")), None)
                contents.append({"trackRef": track["ref"], "trackIdentity": track["objectIdentity"], "slot": {key: slot.get(key) for key in ("ref", "parentRef", "trackRef", "objectIdentity", "clipRef", "empty")} if slot is not None else None, "clip": remote_module._without_fields(clip, remote_module._VOLATILE_CLIP_FIELDS)})
            return hashlib.sha256(mapper._bounded_canonical({"scene": {key: scene.get(key) for key in ("ref", "parentRef", "objectIdentity", "name", "triggerable")}, "contents": contents}).encode("utf-8")).hexdigest()
        for index in (0, 2, 5, 6, 7):
            reference = whole["tracks"][index]["ref"]
            built, fingerprint = self.built_tracks(mapper, lambda: mapper._ownership_fingerprint(reference))
            self.assertEqual(fingerprint, old_track(reference)); self.assertEqual(built, [index])
        for scene in whole["scenes"]:
            built, fingerprint = self.built_tracks(mapper, lambda: mapper._ownership_fingerprint(scene["ref"]))
            self.assertEqual(fingerprint, old_scene(scene["ref"])); self.assertEqual(built, [])

    def test_mutations_read_only_the_tracks_they_name(self):
        song = rich_song(tracks=30, playing=False); song.view = None; mapper = LiveObjectMapper(song, provenance="real-live"); whole = mapper.snapshot()
        # The previews (the host's reads) come first; only the mutations are counted.
        track = whole["tracks"][9]; mixer = track["mixer"]
        state = {field: mixer.get(field) for field in ("volume", "pan", "mute", "solo", "cueVolume", "sends")}
        mixer_args = {"ref": track["ref"], "volume": 0.75, "expectedObjectIdentity": track["objectIdentity"], "expectedVolumeIdentity": mixer["volumeIdentity"], "expectedPanIdentity": mixer["panIdentity"], "expectedCueIdentity": mixer["cueIdentity"], "expectedSendIdentities": mixer["sendIdentities"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode()).hexdigest()}
        clip_args = ControlSurfaceTests.clip_creation_args(mapper, whole["tracks"][11]["ref"], 0, kind="midi", name="Targeted", length=4)
        parameter = whole["tracks"][4]["devices"][1]["parameters"][0]
        parameter_args = {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(mapper, parameter["ref"])}
        counter = ReadCounter(song.tracks)
        mapper.invoke("mixer.set", mixer_args); mapper.invoke("device.parameter.set", parameter_args)
        self.assertEqual(set(counter.reads), {4, 9}, counter.reads)
        # A creation also records the Set's identity topology (each track's slots, devices and
        # Arrangement clips by identity, no rows) to prove an exact rollback if its ownership can't attach.
        counter.reads.clear()
        created = mapper.invoke("clip.create", clip_args, "targeted-transaction")
        self.assertTrue(all(reads <= {"clip_slots", "devices", "arrangement_clips"} for index, reads in counter.reads.items() if index != 11), counter.reads)
        note_args = {"ref": created["ref"], "notes": [{"pitch": 60, "start": 0, "duration": 1, "velocity": 100, "channel": 1}], **ControlSurfaceTests.note_authority(mapper, created["ref"])}
        counter.reads.clear()
        mapper.invoke("note.add-batch", note_args, "targeted-transaction")
        self.assertEqual(set(counter.reads), {11}, counter.reads)
        self.assertEqual(song.tracks[9].mixer_device.volume.value, 0.75); self.assertEqual(song.tracks[4].devices[1].parameters[0].value, 0.75)


class ScopedAuthorityDigestTests(unittest.TestCase):
    """WS2.1: a mutation's authority digest costs what it names: its references' rows, the light
    structure and the transport; playback, locators and Arrangement clips only where they matter."""

    def parameter_args(self, mapper, whole, track=4, device=1):
        parameter = whole["tracks"][track]["devices"][device]["parameters"][0]
        return {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(mapper, parameter["ref"])}

    def test_a_parameter_change_is_fenced_by_its_own_track_and_reads_nothing_else(self):
        song = rich_song(tracks=30, playing=False); song.view = None; mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        args = self.parameter_args(mapper, whole)
        counter = ReadCounter(song.tracks); builds = []; build = LiveObjectMapper._build_snapshot
        with patch.object(LiveObjectMapper, "_build_snapshot", lambda self, arguments=None: builds.append(1) or build(self, arguments)):
            digest = _authority_state_digest(mapper, args, "device.parameter.set")
        self.assertEqual((builds, set(counter.reads)), ([], {4}), counter.reads)
        song.tracks[20].devices[0].parameters[0].value = 0.25
        song.tracks[7].clip_slots[0].clip = FakeClip(4.0); song.tracks[7].fired_slot_index = 0
        self.assertEqual(_authority_state_digest(mapper, args, "device.parameter.set"), digest, "other tracks and what's queued don't concern it")
        song.tracks[4].devices[1].parameters[0].value = 0.25
        self.assertNotEqual(_authority_state_digest(mapper, args, "device.parameter.set"), digest, "its own parameter does")
        song.tracks[4].devices[1].parameters[0].value = 0.5
        song.tempo = 99.0
        self.assertNotEqual(_authority_state_digest(mapper, args, "device.parameter.set"), digest, "and the transport does")

    def test_playback_binds_the_operations_about_playback(self):
        song = rich_song(playing=False); mapper = LiveObjectMapper(song); set_row = mapper.snapshot()["set"]
        stop = {"setRef": set_row["ref"], "action": "stop", "expectedObjectIdentity": set_row["objectIdentity"], "expectedRevision": mapper._playback()["revision"]}
        rename = {"ref": set_row["ref"]}
        before = {operation: _authority_state_digest(mapper, stop, operation) for operation in ("transport.action", "track.set")}
        song.tracks[3].fired_slot_index = 2
        self.assertNotEqual(_authority_state_digest(mapper, stop, "transport.action"), before["transport.action"])
        self.assertEqual(_authority_state_digest(mapper, stop, "track.set"), before["track.set"])
        self.assertNotEqual(_authority_state_digest(mapper, rename), _authority_state_digest(mapper, rename, "track.set"), "no operation named: every part is bound")

    def test_locators_and_arrangement_clips_bind_only_operations_about_them(self):
        song = rich_song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        locator_args = {"name": "Chorus", "position": 32.0}
        on_two = {"trackRef": whole["tracks"][2]["ref"], "position": 8.0, "length": 4.0, "name": "New"}
        on_three = {**on_two, "trackRef": whole["tracks"][3]["ref"]}
        parameter = self.parameter_args(mapper, whole)
        def digests(): return (_authority_state_digest(mapper, locator_args, "locator.add"), _authority_state_digest(mapper, on_two, "arrangement.clip.create"), _authority_state_digest(mapper, on_three, "arrangement.clip.create"), _authority_state_digest(mapper, parameter, "device.parameter.set"))
        before = digests()
        song.cue_points.append(FakeLocator(24.0, "Bridge"))
        after_locator = digests()
        self.assertEqual([a == b for a, b in zip(before, after_locator)], [False, True, True, True])
        song.tracks[2].arrangement_clips.append(FakeClip(1.0))
        after_clip = digests()
        self.assertEqual([a == b for a, b in zip(after_locator, after_clip)], [True, False, True, True])

    def test_no_collection_size_refuses_it_and_no_notes_or_markers_are_read(self):
        class Unreadable(FakeClip):
            def get_all_notes_extended(self): raise RuntimeError("notes are not read for an authority")
            @property
            def warp_markers(self): raise RuntimeError("markers are not read for an authority")
        song = rich_song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        song.cue_points = [FakeLocator(float(index), f"L{index}") for index in range(300)]
        song.tracks[2].arrangement_clips = [Unreadable(1.0) for _ in range(300)]
        args = {"trackRef": whole["tracks"][2]["ref"], "position": 8.0, "length": 4.0, "name": "New"}
        for operation in ("arrangement.clip.create", "locator.add", None):
            self.assertRegex(_authority_state_digest(mapper, args, operation), r"^[a-f0-9]{64}$")

    def test_authority_digest_answers_the_digest_a_mutation_is_checked_against(self):
        song = rich_song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot(); args = self.parameter_args(mapper, whole)
        request = {"operation": "device.parameter.set", "args": args}
        validate_operation_payload("authority.digest", "request", request)
        result = mapper.invoke("authority.digest", request)
        validate_operation_payload("authority.digest", "result", result)
        self.assertEqual(result, {"stateDigest": _authority_state_digest(mapper, args, "device.parameter.set"), "epoch": mapper.refs.epoch})
        self.assertTrue(mapper._operation_supported("authority.digest"))
        # A read: no preflight, prepare or authority token.
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, frame: AbletonMcpBridge._dispatch_main_for(method, frame, mapper))
        unsigned = remote.bound({"version": PROTOCOL, "id": "digest-1", "method": "invoke", "operation": "authority.digest", "args": request, "nonce": "digest-nonce-0001", "sequence": 1})
        answer = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertTrue(answer["ok"], answer); self.assertEqual(answer["result"]["stateDigest"], result["stateDigest"])
        with self.assertRaisesRegex(ValueError, "authority digest arguments are invalid"): mapper.invoke("authority.digest", {"operation": "device.parameter.set", "args": "not an object"})


def immediate_bridge(song=None, provenance="fake-live"):
    """A bridge whose Live-thread queue runs work at once, as the pump does inline."""
    bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song or FakeSong(), provenance=provenance)
    bridge._executed_mutations = {}; bridge._pending_mutations = {}; bridge._retired_mutation_keys = {}; bridge._finalized_transactions = set(); bridge._executed_lock = threading.Lock()
    class ImmediateQueue:
        def submit(self, action, deadline_ms=None, on_cancel=None): return action()
    bridge.queue = ImmediateQueue()
    return bridge


class SingleRequestMutationTests(unittest.TestCase):
    """WS2.2: `mutate` checks and applies a change in one Live-thread callback, through the same
    idempotency ledger as invoke."""

    def parameter_request(self, bridge, key="mutate-key-0001", value=0.75, digest=True):
        parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        args = {"ref": parameter["ref"], "value": value, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(bridge.mapper, parameter["ref"])}
        request = {"operation": "device.parameter.set", "transactionId": "transaction-mutate", "idempotencyKey": key, "args": args}
        if digest: request["stateDigest"] = bridge.mapper.invoke("authority.digest", {"operation": "device.parameter.set", "args": args})["stateDigest"]
        return request

    def test_a_change_applies_in_one_request_and_a_retry_returns_what_was_recorded(self):
        bridge = immediate_bridge(); request = self.parameter_request(bridge); calls = []
        original = bridge.mapper.invoke
        bridge.mapper.invoke = lambda *args, **kwargs: calls.append(args[0]) or original(*args, **kwargs)
        result = bridge._dispatch_with_holder("mutate", request, {})
        self.assertEqual((result["value"], result["revision"]), (0.75, 2)); validate_operation_payload("device.parameter.set", "result", result)
        self.assertEqual(calls, ["device.parameter.set"]); self.assertEqual(bridge._pending_mutations, {})
        # Live changed (by this very change), so the preview's digest no longer matches: the retry still gets the recorded result.
        self.assertEqual(bridge._dispatch_with_holder("mutate", request, {}), result); self.assertEqual(calls, ["device.parameter.set"])
        with self.assertRaisesRegex(ValueError, "conflicts with an executed mutation"): bridge._dispatch_with_holder("mutate", {**request, "args": {**request["args"], "value": 0.25}}, {})
        self.assertEqual(bridge._dispatch_with_holder("retire", {"transactionId": "transaction-mutate", "deadlineMs": int(time.time() * 1000) + 5000}, {}), {"retired": 1})
        with self.assertRaisesRegex(ValueError, "replay authority has been retired"): bridge._dispatch_with_holder("mutate", request, {})

    def test_a_stale_preview_refuses_and_nothing_changes(self):
        bridge = immediate_bridge(); request = self.parameter_request(bridge)
        parameter = bridge.mapper.song.tracks[0].devices[0].parameters[0]; parameter.value = 0.25
        with self.assertRaisesRegex(ValueError, "^Live state changed since the preview; nothing changed$"): bridge._dispatch_with_holder("mutate", request, {})
        self.assertEqual(parameter.value, 0.25); self.assertEqual((bridge._pending_mutations, bridge._executed_mutations), ({}, {}))
        # Without a digest, the operation's own fences decide (its revision and identities still match).
        parameter.value = 0.5
        self.assertEqual(bridge._dispatch_with_holder("mutate", {key: value for key, value in self.parameter_request(bridge, key="mutate-key-0002").items() if key != "stateDigest"}, {})["value"], 0.75)

    def test_every_refusal_before_a_change_runs_says_nothing_changed(self):
        """The host reports these as refused, not uncertain: its arguments, the ledger, the fences
        (ownership, the preview's state), and a queue that never ran it."""
        unrun = "; nothing changed$"
        bridge = immediate_bridge(provenance="real-live"); mapper = bridge.mapper
        def refused(pattern, request, holder=None):
            with self.assertRaisesRegex(ValueError, pattern + ".*" + unrun): bridge._dispatch_with_holder("mutate", request, holder or {})
        refused("read-only operations are invoked, not mutated", {"operation": "song.read", "transactionId": "transaction-read", "idempotencyKey": "read-key-0001", "args": {"setRef": "x"}})
        refused("mutation transaction identity is required", {"operation": "track.rename", "idempotencyKey": "rename-key-0001", "args": {}})
        # The ledger: a key spent on another change, a retired key.
        request = self.parameter_request(bridge); parameter = mapper.song.tracks[0].devices[0].parameters[0]
        self.assertEqual(bridge._dispatch_with_holder("mutate", request, {})["value"], 0.75)
        refused("idempotency key conflicts with an executed mutation", {**request, "args": {**request["args"], "value": 0.25}})
        bridge._dispatch_with_holder("retire", {"transactionId": "transaction-mutate", "deadlineMs": int(time.time() * 1000) + 5000}, {})
        refused("mutation replay authority has been retired", request); self.assertEqual(parameter.value, 0.75)
        # Ownership: another transaction's, a changed creation, a lower one before a higher one.
        transaction = "transaction-owner"; made = [mapper.invoke("track.create", {"name": name, "kind": "midi", "index": index, "expectedStructureRevision": mapper._structure_revision()}, transaction) for index, name in ((1, "Owned A"), (2, "Owned B"))]
        delete = lambda row, key, owner=transaction: {"operation": "track.delete", "transactionId": owner, "idempotencyKey": key, "ownershipToken": row["ownershipToken"], "args": {"ref": row["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": row["objectIdentity"]}}
        refused("destructive cleanup lacks exact transaction-owned authority", delete(made[1], "delete-key-0001", "transaction-other"))
        refused("transaction-owned structure cleanup must proceed from the highest positional authority", delete(made[0], "delete-key-0002"))
        owned_b = mapper.song.tracks[2]; mapper.song.tracks[2] = FakeTrack()
        refused("the object at this reference isn't the one this transaction made any more; cleanup refused", delete(made[1], "delete-key-0003"))
        mapper.song.tracks[2] = owned_b
        self.assertEqual([track.name for track in mapper.song.tracks], ["Drums", "Owned A", "Owned B"]); self.assertEqual(bridge._pending_mutations, {})
        # A queue that refused it before Live's thread ran it (its deadline had passed).
        queued = immediate_bridge(); queued.queue = remote_module._MainThreadQueue()
        late = {**self.parameter_request(queued, key="late-key-0001"), "deadlineMs": int(time.time() * 1000) - 1}
        with self.assertRaisesRegex(TimeoutError, "deadline expired" + unrun): queued._dispatch_with_holder("mutate", late, {})
        self.assertEqual((queued.mapper.song.tracks[0].devices[0].parameters[0].value, queued._pending_mutations), (0.5, {}))
        # And it says so on the wire, however long the reason.
        self.assertTrue(remote_module._failure_summary(ValueError("x" * 300 + remote_module.UNRUN_SUFFIX)).endswith("; nothing changed"))
        self.assertEqual(remote_module._failure_summary(ValueError("short" + remote_module.UNRUN_SUFFIX)), "request failed: short; nothing changed")

    def test_reads_and_owned_deletions_keep_their_rules(self):
        bridge = immediate_bridge(provenance="real-live")
        with self.assertRaisesRegex(ValueError, "read-only operations are invoked, not mutated"): bridge._dispatch_with_holder("mutate", {"operation": "song.read", "transactionId": "transaction-read", "idempotencyKey": "read-key-0001", "args": {"setRef": "x"}}, {})
        created = bridge.mapper.invoke("scene.create", {"name": "Owned", "index": 1, "expectedStructureRevision": bridge.mapper._structure_revision()}, "transaction-owner")
        delete = {"operation": "scene.delete", "transactionId": "transaction-other", "idempotencyKey": "delete-key-0001", "args": {"ref": created["ref"], "expectedStructureRevision": bridge.mapper._structure_revision(), "expectedObjectIdentity": created["objectIdentity"]}}
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): bridge._dispatch_with_holder("mutate", delete, {})
        self.assertEqual(len(bridge.mapper.song.scenes), 2)
        owned = {**delete, "transactionId": "transaction-owner", "idempotencyKey": "delete-key-0002", "ownershipToken": created["ownershipToken"]}
        self.assertEqual(bridge._dispatch_with_holder("mutate", owned, {}), {"deleted": created["ref"]}); self.assertEqual(len(bridge.mapper.song.scenes), 1)

    def test_the_wire_checks_a_mutation_and_its_result_against_the_operations_schema(self):
        bridge = immediate_bridge(); request = self.parameter_request(bridge)
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, frame: bridge._dispatch_with_holder(method, frame, {}))
        def send(sequence, **fields):
            unsigned = remote.bound({"version": PROTOCOL, "id": f"mutate-{sequence}", "method": "mutate", "nonce": f"mutate-nonce-{sequence:04d}", "sequence": sequence, **fields})
            return remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        answer = send(1, **request)
        self.assertTrue(answer["ok"], answer); self.assertEqual(answer["result"]["value"], 0.75)
        for sequence, broken in enumerate(({**request, "idempotencyKey": "short"}, {**request, "stateDigest": "0" * 63}, {key: value for key, value in request.items() if key != "transactionId"}, {**request, "args": {**request["args"], "unknown": 1}}, {**request, "operation": "snapshot"}), 2):
            self.assertFalse(send(sequence, **broken)["ok"], broken)
        unsigned = remote.bound({"version": PROTOCOL, "id": "invoke-digest", "method": "invoke", "operation": "status", "stateDigest": "0" * 64, "nonce": "invoke-digest-0001", "sequence": 20})
        self.assertFalse(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})["ok"], "a state digest belongs to mutate alone")


class FakeUndoSong(FakeSong):
    def __init__(self):
        super().__init__(); self.calls = []; self.can_undo = True; self.can_redo = False
    def begin_undo_step(self): self.calls.append("begin")
    def end_undo_step(self): self.calls.append("end")
    def undo(self): self.calls.append("undo"); self.can_redo = True
    def redo(self): self.calls.append("redo"); self.can_redo = False


class PythonRunTests(unittest.TestCase):
    def setUp(self):
        self.song = FakeUndoSong()
        self.song.tempo = 120
        self.song.tracks[0].clip_slots[0].create_clip(4)
        self.mapper = LiveObjectMapper(self.song)
        self.application = types.SimpleNamespace(marker="app")
        self.live = types.SimpleNamespace(
            marker="Live", Application=types.SimpleNamespace(get_application=lambda: self.application),
            Track=types.SimpleNamespace(Track=FakeTrack), Scene=types.SimpleNamespace(Scene=FakeScene),
            ClipSlot=types.SimpleNamespace(ClipSlot=FakeSlot), Clip=types.SimpleNamespace(Clip=FakeClip),
            Device=types.SimpleNamespace(Device=FakeDevice), DeviceParameter=types.SimpleNamespace(DeviceParameter=FakeParameter),
        )
        self.live_patch = patch.dict(sys.modules, {"Live": self.live})
        self.live_patch.start()
        self.addCleanup(self.live_patch.stop)

    def run_python(self, code, **args):
        request = {"code": code, **args}
        validate_operation_payload("python.run", "request", request)
        result = self.mapper.invoke("python.run", request)
        validate_operation_payload("python.run", "result", result)
        json.dumps(result, allow_nan=False)
        return result

    def test_eval_has_the_live_namespace_and_json_values(self):
        result = self.run_python("(Live.marker, song.tempo, app.marker, obj is None, bridge.song is song)", mode="eval")
        self.assertEqual(result, {"ok": True, "result": ["Live", 120, "app", True, True], "stdout": "", "error": None})
        self.assertEqual(self.song.calls, ["begin", "end"])
        self.assertTrue(self.mapper._operation_supported("python.run"))
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("python.run"))
        with patch.dict(sys.modules, {"Live": None}): self.assertFalse(self.mapper._operation_supported("python.run"))

    def test_exec_captures_stdout_returns_result_and_mutates_without_rollback(self):
        result = self.run_python("print('renaming')\nsong.tracks[0].name = 'Python Bass'\nresult = {'name': song.tracks[0].name, 'tuple': (1, 2), 'set': {3}, 'frozen': frozenset([4])}")
        self.assertEqual(result, {"ok": True, "result": {"name": "Python Bass", "tuple": [1, 2], "set": [3], "frozen": [4]}, "stdout": "renaming\n", "error": None})
        self.assertEqual(self.song.tracks[0].name, "Python Bass")
        self.assertIsNone(self.run_python("print('no result')")["result"])
        # Exec globals and locals are shared, including functions defined by the script.
        self.assertEqual(self.run_python("x = 3\ndef answer(): return x + song.tempo\nresult = answer()")["result"], 123)

    def test_live_objects_return_refs_consumed_by_the_typed_mapper(self):
        result = self.run_python("(song, song.tracks[0], song.scenes[0], song.tracks[0].devices[0], song.tracks[0].devices[0].parameters[0], song.tracks[0].clip_slots[0], song.tracks[0].clip_slots[0].clip)", mode="eval")
        self.assertTrue(result["ok"], result)
        objects = [self.song, self.song.tracks[0], self.song.scenes[0], self.song.tracks[0].devices[0], self.song.tracks[0].devices[0].parameters[0], self.song.tracks[0].clip_slots[0], self.song.tracks[0].clip_slots[0].clip]
        for row, obj, kind in zip(result["result"], objects, ["set", "track", "scene", "device", "parameter", "clip_slot", "clip"]):
            self.assertEqual(set(row), {"ref", "type", "name"})
            self.assertIn(f":{kind}:", row["ref"])
            self.assertIs(self.mapper.refs.get(row["ref"]), obj)
            if kind != "clip_slot": self.assertEqual(self.mapper.get(row["ref"])["ref"], row["ref"])
        reference = result["result"][1]["ref"]
        self.assertEqual(self.run_python("obj.name", mode="eval", ref=reference)["result"], "Drums")
        # A script can shift the positions registered by earlier discovery.
        shifted = self.run_python("song.create_midi_track(0)\nresult = song.tracks")
        self.assertTrue(shifted["ok"], shifted)
        self.assertEqual([row["ref"] for row in shifted["result"]], [f"{self.mapper.refs.epoch}:track:0", f"{self.mapper.refs.epoch}:track:1"])
        self.assertEqual(self.mapper.get(shifted["result"][1]["ref"])["name"], "Drums")

    def test_exceptions_and_exit_are_data_and_restore_stdout_trace_and_undo(self):
        previous_stdout, previous_trace = sys.stdout, sys.gettrace()
        for expression, name, message in [("raise ValueError('broken')", "ValueError", "broken"), ("raise SystemExit(9)", "SystemExit", "9"), ("raise KeyboardInterrupt('stop')", "KeyboardInterrupt", "stop"), ("raise GeneratorExit('exit')", "GeneratorExit", "exit")]:
            with self.subTest(name=name):
                result = self.run_python("song.tempo = 126\nprint('before failure')\n" + expression)
                self.assertFalse(result["ok"])
                self.assertIsNone(result["result"])
                self.assertEqual(result["stdout"], "before failure\n")
                self.assertEqual((result["error"]["type"], result["error"]["message"]), (name, message))
                self.assertIn("<python.run>", result["error"]["traceback"])
                self.assertEqual(self.song.tempo, 126, "failed scripts keep the changes they made")
                self.assertEqual(self.song.calls[-2:], ["begin", "end"])
                self.assertIs(sys.stdout, previous_stdout)
                self.assertIs(sys.gettrace(), previous_trace)
                self.assertIsNone(self.mapper._undo_step)
        syntax = self.run_python("result =")
        self.assertEqual(syntax["error"]["type"], "SyntaxError")

    def test_timeout_interrupts_a_loop_and_cleans_up(self):
        started = time.perf_counter()
        result = self.run_python("print('started')\nwhile True: pass", timeoutMs=10)
        self.assertLess(time.perf_counter() - started, 1)
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["type"], "TimeoutError")
        self.assertIn("10 ms", result["error"]["message"])
        self.assertEqual(result["stdout"], "started\n")
        self.assertEqual(self.song.calls, ["begin", "end"])
        self.assertIsNone(self.mapper._undo_step)
        self.assertEqual(self.run_python("2 + 2", mode="eval")["result"], 4)

    def test_an_open_undo_step_is_kept_and_a_previous_trace_restored(self):
        step = self.mapper.invoke("undo.step.begin", {"label": "Plan"})
        previous = sys.gettrace()
        def prior_trace(frame, event, arg): return prior_trace
        try:
            sys.settrace(prior_trace)
            self.assertTrue(self.run_python("result = 1")["ok"])
            self.assertIs(sys.gettrace(), prior_trace)
            self.assertFalse(self.run_python("raise SystemExit()", timeoutMs=10)["ok"])
            self.assertIs(sys.gettrace(), prior_trace)
        finally:
            sys.settrace(previous)
        self.assertEqual(self.song.calls, ["begin"])
        self.assertEqual(self.mapper._undo_step["stepId"], step["stepId"])
        self.mapper.invoke("undo.step.end", {"stepId": step["stepId"]})
        self.assertEqual(self.song.calls, ["begin", "end"])

    def test_non_json_results_and_unprintable_exceptions_are_data(self):
        for code in ["float('nan')", "float('inf')", "2 ** 100", "object()"]:
            with self.subTest(code=code): self.assertFalse(self.run_python(code, mode="eval")["ok"])
        self.assertFalse(self.run_python("result = []; result.append(result)")["ok"])
        bad_error = self.run_python("class BadError(BaseException):\n def __str__(self): raise SystemExit()\nraise BadError()")
        self.assertEqual(bad_error["error"]["type"], "BadError")
        self.assertEqual(bad_error["error"]["message"], "Error message unavailable")

    def test_legacy_note_removal_is_refused_before_it_runs(self):
        # Live stops a script calling these to ask the producer, holding the bridge until someone answers.
        clip = "song.tracks[0].clip_slots[0].clip"
        for code, names in [(f"song.tempo = 126\n{clip}.remove_notes(0.0, 0, 4.0, 128)", "remove_notes is"),
                            (f"def rewrite(c):\n    c.select_all_notes()\n    c.replace_selected_notes(())\nsong.tempo = 126\nrewrite({clip})", "replace_selected_notes is"),
                            (f"song.tempo = 126\ngetattr({clip}, 'remove_notes')(0.0, 0, 4.0, 128)\n{clip}.replace_selected_notes(())", "remove_notes and replace_selected_notes are")]:
            with self.subTest(code=code):
                result = self.run_python(code)
                self.assertFalse(result["ok"])
                self.assertEqual(result["error"]["type"], "ValueError")
                self.assertTrue(result["error"]["message"].startswith(f"{names} Live's old way to remove notes"), result["error"]["message"])
                self.assertIn("remove_notes_extended(from_pitch, pitch_span, from_time, time_span)", result["error"]["message"])
                self.assertEqual(self.song.tempo, 120, "nothing in the script ran")
                self.assertIsNone(self.mapper._undo_step)
        # Live 11's calls, and the old names in a comment, run.
        allowed = self.run_python(f"# not remove_notes or replace_selected_notes\nresult = [callable(getattr({clip}, name, None)) for name in ('remove_notes_extended', 'remove_notes_by_id')]")
        self.assertTrue(allowed["ok"], allowed)

    def test_authenticated_invoke_needs_no_authority_and_runs_on_the_live_queue(self):
        self.assertIn("python.run", remote_module._AUTHORITY_FREE_INVOKES)
        self.assertNotIn("python.run", remote_module._READ_ONLY_INVOKES)
        bridge = immediate_bridge(self.song)
        bridge.queue = _MainThreadQueue()
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, frame: bridge._dispatch_with_holder(method, frame, {}))
        unsigned = remote.bound({"version": PROTOCOL, "id": "python", "method": "invoke", "operation": "python.run", "args": {"code": "import threading\nprint(threading.get_ident())\nraise SystemExit('exit')"}, "nonce": "python-nonce-0001", "sequence": 1})
        replies = []
        worker = threading.Thread(target=lambda: replies.append(remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})))
        worker.start()
        deadline = time.perf_counter() + 1
        while bridge.queue.items.empty() and time.perf_counter() < deadline: time.sleep(0.001)
        self.assertEqual(bridge.queue.drain(), 1)
        worker.join(1)
        self.assertFalse(worker.is_alive())
        self.assertTrue(replies[0]["ok"], replies)
        result = replies[0]["result"]
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["type"], "SystemExit")
        self.assertEqual(result["stdout"], str(threading.get_ident()) + "\n")


class LiveUndoTests(unittest.TestCase):
    """WS3.2/WS3.4: one Live undo step around a plan, never left open; Live's own undo and redo."""

    def test_an_undo_step_opens_closes_and_closes_a_previous_one_first(self):
        song = FakeUndoSong(); mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("undo.step.begin") and mapper._operation_supported("undo.step.end"))
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("undo.step.begin"))
        opened = mapper.invoke("undo.step.begin", {"label": "Kumi: make it warmer"}); validate_operation_payload("undo.step.begin", "result", opened)
        self.assertEqual((opened["open"], opened["closedPrevious"], song.calls), (True, False, ["begin"]))
        self.assertAlmostEqual(opened["expiresAt"], time.time() * 1000 + 120000, delta=2000)
        second = mapper.invoke("undo.step.begin", {"timeoutMs": 5000})
        self.assertTrue(second["closedPrevious"]); self.assertEqual(song.calls, ["begin", "end", "begin"])
        other = mapper.invoke("undo.step.end", {"stepId": opened["stepId"]}); validate_operation_payload("undo.step.end", "result", other)
        self.assertEqual(other, {"closed": False, "stepId": second["stepId"], "reason": "other-step"}); self.assertEqual(song.calls[-1], "begin")
        ended = mapper.invoke("undo.step.end", {"stepId": second["stepId"]})
        self.assertEqual(ended, {"closed": True, "stepId": second["stepId"], "reason": "ended"}); self.assertEqual(song.calls[-1], "end")
        self.assertEqual(mapper.invoke("undo.step.end", {}), {"closed": False, "stepId": None, "reason": "not-open"})
        for invalid in ({"timeoutMs": 999}, {"timeoutMs": 3600001}, {"label": ""}, {"other": 1}):
            with self.assertRaises(ValueError): mapper.invoke("undo.step.begin", invalid)

    def test_undo_steps_need_no_mutation_authority_but_are_not_reads(self):
        self.assertFalse(remote_module._mutation_authority_required("undo.step.begin")); self.assertFalse(remote_module._mutation_authority_required("undo.step.end"))
        self.assertTrue(remote_module._AUTHORITY_FREE_INVOKES.isdisjoint(remote_module._READ_ONLY_INVOKES))
        bridge = immediate_bridge(FakeUndoSong())
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, frame: bridge._dispatch_with_holder(method, frame, {}))
        unsigned = remote.bound({"version": PROTOCOL, "id": "undo-begin", "method": "invoke", "operation": "undo.step.begin", "args": {}, "nonce": "undo-begin-000001", "sequence": 1})
        answer = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertTrue(answer["ok"], answer); self.assertTrue(answer["result"]["open"])

    def test_a_guard_closes_the_step_at_its_deadline_on_close_reconnect_and_shutdown(self):
        song = FakeUndoSong(); bridge = immediate_bridge(song); holder = {}
        opened = bridge._dispatch_with_holder("invoke", {"operation": "undo.step.begin", "args": {"timeoutMs": 1000}}, holder)
        with patch("ableton_mcp_remote_script.time.time", return_value=(opened["expiresAt"] - 1) / 1000): bridge.mapper.undo_step_tick()
        self.assertEqual(song.calls, ["begin"])
        with patch("ableton_mcp_remote_script.time.time", return_value=opened["expiresAt"] / 1000): bridge.mapper.undo_step_tick()
        self.assertEqual(song.calls, ["begin", "end"]); self.assertIsNone(bridge.mapper._undo_step)
        # Its connection closing closes it; another connection's doesn't.
        bridge._connections = []; bridge._clients = set()
        bridge._dispatch_with_holder("invoke", {"operation": "undo.step.begin", "args": {}}, holder)
        class Closable:
            def close(self): pass
        bridge._close(types.SimpleNamespace(holder={}, socket=Closable())); self.assertEqual(song.calls[-1], "begin")
        bridge._close(types.SimpleNamespace(holder=holder, socket=Closable())); self.assertEqual(song.calls[-1], "end")
        bridge._dispatch_with_holder("mutate", {"operation": "undo.step.begin", "transactionId": "transaction-undo", "idempotencyKey": "undo-key-0001", "args": {}}, holder)
        bridge.mapper.invoke("session.reconnect", {}); self.assertEqual(song.calls[-2:], ["begin", "end"])
        # And the bridge shutting down.
        import socket as _socket
        probe = _socket.socket(); probe.bind(("127.0.0.1", 0)); port = probe.getsockname()[1]; probe.close()
        instance = FakeInstance(); instance.song = FakeUndoSong(); live = AbletonMcpBridge(instance, {"host": "127.0.0.1", "port": port, "secret": "x" * 40})
        live.mapper.invoke("undo.step.begin", {}); live.update_display(); self.assertEqual(instance.song.calls, ["begin"])
        live.disconnect(); self.assertEqual(instance.song.calls, ["begin", "end"])

    def test_lives_own_undo_and_redo_run_only_when_there_is_something_to_undo(self):
        song = FakeUndoSong(); mapper = LiveObjectMapper(song)
        self.assertTrue(mapper._operation_supported("song.undo") and mapper._operation_supported("song.redo"))
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("song.undo"))
        self.assertTrue(remote_module._mutation_authority_required("song.undo") and remote_module._mutation_authority_required("song.redo"))
        undone = mapper.invoke("song.undo", {}); validate_operation_payload("song.undo", "result", undone)
        self.assertEqual((undone, song.calls), ({"done": True, "canUndo": True, "canRedo": True}, ["undo"]))
        self.assertEqual(mapper.invoke("song.redo", {}), {"done": True, "canUndo": True, "canRedo": False})
        song.can_undo = False
        self.assertEqual(mapper.invoke("song.undo", {}), {"done": False, "canUndo": False, "canRedo": False}); self.assertEqual(song.calls, ["undo", "redo"])
        self.assertEqual(mapper.invoke("song.redo", {})["done"], False); self.assertEqual(song.calls, ["undo", "redo"])
        with self.assertRaises(ValueError): mapper.invoke("song.undo", {"steps": 2})


class MutateOverTheWireTests(_BridgeSocketFixture, unittest.TestCase):
    def test_a_previewed_change_is_one_round_trip(self):
        client, channel = self.connect()
        mapper = self.bridge.mapper; parameter = mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        args = {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(mapper, parameter["ref"])}
        client.sendall(self.frame(channel, 1, method="invoke", operation="authority.digest", args={"operation": "device.parameter.set", "args": args}))
        digest = self.read_lines(client, 1)[0]["result"]["stateDigest"]
        client.sendall(self.frame(channel, 2, method="mutate", operation="device.parameter.set", args=args, transactionId="transaction-wire", idempotencyKey="wire-key-0001", stateDigest=digest))
        answer = self.read_lines(client, 1)[0]
        self.assertTrue(answer["ok"], answer); self.assertEqual(answer["result"]["value"], 0.75)
        self.assertEqual(mapper.song.tracks[0].devices[0].parameters[0].value, 0.75)


class ExplicitDeletionTests(unittest.TestCase):
    """WS3.3: a producer-confirmed deletion of a clip, Arrangement clip, scene, track or locator names
    explicitDeletion instead of a creating transaction's token, and every identity fence still holds."""

    def test_clips_scenes_tracks_and_locators_delete_on_their_fences_without_ownership(self):
        song = FakeArrangementSong(); song.tracks = [FakeTrack(), FakeTrack(), FakeTrack()]; song.scenes = [FakeScene("A"), FakeScene("B")]
        for index, track in enumerate(song.tracks): track.name = f"T{index}"; track.clip_slots = [FakeSlot(), FakeSlot()]; track.delete_clip = lambda clip, track=track: track.arrangement_clips.remove(clip); track.arrangement_clips = [FakeClip(1.0), FakeClip(2.0)]
        song.tracks[0].clip_slots[0].clip = FakeClip(4.0)
        mapper = LiveObjectMapper(song, provenance="real-live"); snapshot = mapper.snapshot(); transaction = "transaction-explicit"
        clip = snapshot["tracks"][0]["clips"][0]; authority = mapper._session_clip_authority(clip["ref"])
        for operation, args in (("clip.delete", {"ref": clip["ref"], **authority}),):
            with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper.invoke(operation, args, transaction)
        stale = {**authority, "expectedSlotIdentity": "live:replacement"}
        with self.assertRaisesRegex(ValueError, "identity changed"): mapper.invoke("clip.delete", {"ref": clip["ref"], **stale, "explicitDeletion": True}, transaction)
        request = {"ref": clip["ref"], **authority, "explicitDeletion": True}; validate_operation_payload("clip.delete", "request", request)
        self.assertEqual(mapper.invoke("clip.delete", request, transaction), {"deleted": clip["ref"]}); self.assertIsNone(song.tracks[0].clip_slots[0].clip)
        arrangement = mapper.snapshot()["arrangement"]["clips"][1]
        arrangement_request = {"ref": arrangement["ref"], "expectedObjectIdentity": arrangement["objectIdentity"], "expectedAuthorityRevision": mapper._arrangement_clip_authority_revision(arrangement["ref"]), "explicitDeletion": True}
        validate_operation_payload("arrangement.clip.delete", "request", arrangement_request)
        with self.assertRaisesRegex(ValueError, "hierarchy changed"): mapper.invoke("arrangement.clip.delete", {**arrangement_request, "expectedAuthorityRevision": "0" * 64}, transaction)
        self.assertEqual(mapper.invoke("arrangement.clip.delete", arrangement_request, transaction), {"deleted": arrangement["ref"]}); self.assertEqual(len(song.tracks[0].arrangement_clips), 1)
        scene = mapper.snapshot()["scenes"][1]
        scene_request = {"ref": scene["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": scene["objectIdentity"], "explicitDeletion": True}
        with self.assertRaisesRegex(ValueError, "structure changed"): mapper.invoke("scene.delete", {**scene_request, "expectedStructureRevision": "0" * 64}, transaction)
        self.assertEqual(mapper.invoke("scene.delete", scene_request, transaction), {"deleted": scene["ref"]}); self.assertEqual([item.name for item in song.scenes], ["A"])
        track = mapper.snapshot()["tracks"][2]
        track_request = {"ref": track["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": track["objectIdentity"], "explicitDeletion": True}
        with self.assertRaisesRegex(ValueError, "identity changed"): mapper.invoke("track.delete", {**track_request, "expectedObjectIdentity": "live:replacement"}, transaction)
        self.assertEqual(mapper.invoke("track.delete", track_request, transaction), {"deleted": track["ref"]}); self.assertEqual([item.name for item in song.tracks], ["T0", "T1"])
        locator = mapper.snapshot()["arrangement"]["locators"][0]
        locator_request = {"ref": locator["ref"], "expectedObjectIdentity": locator["objectIdentity"], "expectedCollectionRevision": mapper.snapshot()["arrangement"]["locatorRevision"], "explicitDeletion": True}
        with self.assertRaisesRegex(ValueError, "locator collection changed"): mapper.invoke("locator.delete", {**locator_request, "expectedCollectionRevision": "0" * 64}, transaction)
        self.assertEqual(mapper.invoke("locator.delete", locator_request, transaction), {"deleted": locator["ref"]}); self.assertEqual(song.cue_points, [])

    def test_a_group_track_goes_with_every_track_inside_it(self):
        """Live deletes a group with what's inside it, nested groups too: the deletion expects exactly that."""
        def grouped_song(live_keeps_members=False):
            song = FakeSong(); song.tracks = [FakeTrack() for _ in range(6)]
            for index, track in enumerate(song.tracks): track.name = f"T{index}"
            group, nested = song.tracks[1], song.tracks[3]; group.is_foldable = True; nested.is_foldable = True
            song.tracks[2].group_track = group; nested.group_track = group; song.tracks[4].group_track = nested
            def delete_track(index):
                gone = song.tracks[index]
                def inside(track):
                    parent, depth = track, 0
                    while parent is not None and depth < 8:
                        if parent is gone: return True
                        parent, depth = getattr(parent, "group_track", None), depth + 1
                    return False
                song.tracks = [track for track in song.tracks if track is not gone and (live_keeps_members or not inside(track))]
            song.delete_track = delete_track
            return song
        song = grouped_song(); mapper = LiveObjectMapper(song, provenance="real-live"); group = mapper.snapshot()["tracks"][1]
        request = {"ref": group["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": group["objectIdentity"], "explicitDeletion": True}
        self.assertEqual(mapper.invoke("track.delete", request, "transaction-group"), {"deleted": group["ref"]})
        self.assertEqual([track.name for track in song.tracks], ["T0", "T5"])
        # A Live that left the tracks inside behind would have done something else than asked: refused.
        song = grouped_song(live_keeps_members=True); mapper = LiveObjectMapper(song, provenance="real-live"); group = mapper.snapshot()["tracks"][1]
        with self.assertRaisesRegex(ValueError, "did not preserve exact remaining sibling order"): mapper.invoke("track.delete", {"ref": group["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": group["objectIdentity"], "explicitDeletion": True}, "transaction-group")

    def test_a_grouped_track_names_its_group_by_the_group_rows_own_ref(self):
        """A track in a group points at the group's own row, nested groups too: whole and light rows,
        snapshots and discovery pages alike."""
        song = FakeSong(); song.tracks = [FakeTrack() for _ in range(6)]
        group, nested = song.tracks[1], song.tracks[3]; group.is_foldable = True; nested.is_foldable = True
        song.tracks[2].group_track = group; nested.group_track = group; song.tracks[4].group_track = nested
        mapper = LiveObjectMapper(song, provenance="real-live")
        def parents(rows):
            refs = {row["ref"]: index for index, row in enumerate(rows)}
            return [refs.get(row.get("groupTrackRef")) if row.get("groupTrackRef") is not None else None for row in rows]
        expected = [None, None, 1, 1, 3, None]
        self.assertEqual(parents(mapper.snapshot()["tracks"][:6]), expected)
        self.assertEqual(parents(mapper.snapshot({"focus": [0]})["tracks"][:6]), expected)
        self.assertEqual(parents(mapper.discover("track", limit=6)["items"]), expected)
        for index in (2, 4):
            row = mapper.snapshot({"tracks": {"from": index, "count": 1}})["tracks"][0]
            self.assertEqual(row["groupTrackRef"], mapper.snapshot()["tracks"][expected[index]]["ref"])

    def test_objects_an_explicit_deletion_moved_lose_their_ownership(self):
        song = FakeSong(); song.tracks = [FakeTrack(), FakeTrack()]; mapper = LiveObjectMapper(song, provenance="real-live")
        made = mapper.invoke("track.create", {"name": "Made later", "kind": "midi", "index": 2, "expectedStructureRevision": mapper._structure_revision()}, "transaction-maker")
        scene = mapper.invoke("scene.create", {"name": "Scene later", "index": 1, "expectedStructureRevision": mapper._structure_revision()}, "transaction-maker")
        first = mapper.snapshot()["tracks"][0]
        mapper.invoke("track.delete", {"ref": first["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": first["objectIdentity"], "explicitDeletion": True}, "transaction-explicit")
        # The made track is now track 1, not 2: its undo is refused for lacking ownership, not run on another track.
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper._require_cleanup_ownership("track.delete", {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"]}, "transaction-maker", made["ownershipToken"])
        # A scene deletion before the made scene moves it too.
        first_scene = mapper.snapshot()["scenes"][0]
        mapper.invoke("scene.delete", {"ref": first_scene["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": first_scene["objectIdentity"], "explicitDeletion": True}, "transaction-explicit")
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper._require_cleanup_ownership("scene.delete", {"ref": scene["ref"], "expectedObjectIdentity": scene["objectIdentity"]}, "transaction-maker", scene["ownershipToken"])
        self.assertEqual(mapper._owned_cleanup_tokens, {})

    def test_an_owned_deletion_is_unchanged(self):
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live")
        made = mapper.invoke("scene.create", {"name": "Owned", "index": 1, "expectedStructureRevision": mapper._structure_revision()}, "transaction-owner")
        args = {"ref": made["ref"], "expectedStructureRevision": mapper._structure_revision(), "expectedObjectIdentity": made["objectIdentity"]}
        with self.assertRaisesRegex(ValueError, "lacks exact transaction-owned authority"): mapper.invoke("scene.delete", args, "transaction-other")
        self.assertEqual(mapper.invoke("scene.delete", args, "transaction-owner", made["ownershipToken"]), {"deleted": made["ref"]})
        self.assertTrue(mapper._owned_cleanup_tokens[made["ownershipToken"]]["deleted"])

    def test_preflight_takes_an_explicit_deletion_without_ownership(self):
        bridge = immediate_bridge(provenance="real-live"); holder = {}
        scene = bridge.mapper.snapshot()["scenes"][0]
        request = {"operation": "scene.delete", "transactionId": "transaction-explicit-preflight", "args": {"ref": scene["ref"], "expectedStructureRevision": bridge.mapper._structure_revision(), "expectedObjectIdentity": scene["objectIdentity"], "explicitDeletion": True}}
        preflight = bridge._dispatch_with_holder("preflight", request, holder)
        prepared = bridge._dispatch_with_holder("prepare", {**request, "preflightToken": preflight["preflightToken"], "confirmation": preflight["confirmation"], "idempotencyKey": "explicit-scene-key"}, holder)
        self.assertEqual(bridge._dispatch_with_holder("invoke", {**request, "authorityToken": prepared["authorityToken"]}, holder), {"deleted": scene["ref"]})


class Listenable:
    """Live's observable properties for fakes: add_<name>_listener / remove_<name>_listener for the
    names in LISTENABLE, and setting one calls its listeners, as Live notifies."""
    LISTENABLE: frozenset = frozenset()

    def __getattr__(self, name):
        for prefix in ("add_", "remove_"):
            if name.startswith(prefix) and name.endswith("_listener") and name[len(prefix):-len("_listener")] in type(self).LISTENABLE:
                target = name[len(prefix):-len("_listener")]
                def change(callback, target=target, adding=prefix == "add_"):
                    listeners = self.__dict__.setdefault("_listeners", {}).setdefault(target, [])
                    listeners.append(callback) if adding else listeners.remove(callback)
                return change
        raise AttributeError(name)

    def __setattr__(self, name, value):
        object.__setattr__(self, name, value)
        self.notify(name)

    def notify(self, name):
        for callback in list(self.__dict__.get("_listeners", {}).get(name, [])): callback()

    def listening(self):
        return sum(len(callbacks) for callbacks in self.__dict__.get("_listeners", {}).values())


class ListenParameter(Listenable, FakeParameter): LISTENABLE = frozenset({"value"})
class ListenScene(Listenable, FakeScene): LISTENABLE = frozenset({"name", "color_index"})
class ListenClip(Listenable, FakeClip): LISTENABLE = frozenset({"name", "color_index"})
class ListenTrackView(Listenable): LISTENABLE = frozenset({"selected_device"})
class ListenView(Listenable): LISTENABLE = frozenset({"selected_track", "selected_scene", "highlighted_clip_slot", "detail_clip", "selected_parameter", "selected_chain"})


class ListenSlot(Listenable, FakeSlot):
    LISTENABLE = frozenset({"has_clip"})
    def __setattr__(self, name, value):
        super().__setattr__(name, value)
        if name == "clip": self.notify("has_clip")


class ListenMixer:
    def __init__(self):
        self.volume = ListenParameter(); self.panning = ListenParameter(); self.sends = [ListenParameter(), ListenParameter()]


class ListenTrack(Listenable, FakeTrack):
    LISTENABLE = frozenset({"name", "color_index", "mute", "solo", "arm", "devices"})


class ListenSong(Listenable, FakeSong):
    LISTENABLE = frozenset({"is_playing", "record_mode", "session_record", "tracks", "scenes", "cue_points"})


def listening_song(tracks=4, scenes=3):
    song = ListenSong(); song.cue_points = []
    rows = []
    for index in range(tracks):
        track = ListenTrack(); track.name = f"T{index}"; track.mute = False; track.solo = False; track.color_index = index
        track.mixer_device = ListenMixer(); track.clip_slots = [ListenSlot() for _ in range(scenes)]
        device = FakeDevice(); device.name = f"D{index}"; device.parameters = [ListenParameter(), ListenParameter()]
        rack = FakeDevice(); rack.name = f"Rack{index}"; rack.can_have_chains = True; nested = FakeDevice(); nested.name = "Nested"; nested.parameters = [ListenParameter()]
        chain = type("Chain", (), {})(); chain.name = "C"; chain.devices = [nested]; chain.mute = False; chain.solo = False; rack.chains = [chain]; rack.macros = []
        track.devices = [device, rack]; track.view = ListenTrackView(); track.view.selected_device = device
        rows.append(track)
    clip = ListenClip(4.0); clip.name = "Loop"; clip.color_index = 5; rows[1].clip_slots[2].clip = clip
    main = ListenTrack(); main.name = "Main"; main.mixer_device = ListenMixer(); main.clip_slots = []; main.devices = []
    song.tracks = rows; song.master_track = main
    song.scenes = [ListenScene(f"S{index}") for index in range(scenes)]
    view = ListenView(); view.selected_track = rows[0]; view.selected_scene = song.scenes[0]; view.highlighted_clip_slot = rows[0].clip_slots[0]; view.detail_clip = None; view.selected_parameter = None; view.selected_chain = None
    song.view = view
    return song


class ListenerEventTests(unittest.TestCase):
    """WS3.5: Live's listeners push selection, names, mixer values, the selected device's parameters
    and structure, with the refs a snapshot gives; only what was asked for is listened to."""

    def events(self, subscription, event_type=None):
        subscription.refresh()
        return [event for event in subscription.drain() if event["type"] != "reset" and (event_type is None or event["type"] == event_type)]

    def test_types_are_probed_without_walking_the_set(self):
        song = listening_song(); counter = ReadCounter(song.tracks)
        self.assertEqual(remote_module._supported_event_types(song), {"reset", "transport", "object", "structure", "selection", "parameter", "name", "mixer"})
        self.assertEqual(counter.reads, {}, "nothing below a track is read to probe")
        self.assertEqual(remote_module._supported_event_types(FakeSong()), {"reset"})
        bridge = object.__new__(AbletonMcpBridge); bridge.mapper = LiveObjectMapper(song); holder = {}
        result = bridge._subscribe_main({"args": {"types": sorted(remote_module._EVENT_TYPES)}}, holder)
        self.assertTrue(result["subscribed"]); holder["subscription"].close()

    def test_only_the_requested_listeners_attach_and_close_detaches_them_all(self):
        song = listening_song(); mapper = LiveObjectMapper(song)
        subscription = _Subscription(mapper, {"name"})
        self.assertTrue(all(track.listening() == 2 for track in song.tracks), "a track's name and colour, nothing else")
        self.assertEqual(song.tracks[0].mixer_device.volume.listening(), 0)
        self.assertEqual(song.tracks[1].clip_slots[2].clip.listening(), 2); self.assertEqual(song.scenes[0].listening(), 2)
        subscription.close()
        everything = song.tracks + [song, song.view] + song.scenes + [song.tracks[1].clip_slots[2].clip] + [slot for track in song.tracks for slot in track.clip_slots] + [song.tracks[0].mixer_device.volume]
        self.assertEqual(sum(item.listening() for item in everything), 0)
        full = _Subscription(mapper, set(remote_module._EVENT_TYPES) - {"reset"}); full.close()
        self.assertEqual(sum(item.listening() for item in everything + [parameter for track in song.tracks for parameter in track.devices[0].parameters]), 0)

    def test_selection_events_name_what_a_snapshot_names(self):
        song = listening_song(); mapper = LiveObjectMapper(song); subscription = _Subscription(mapper, {"selection"})
        track = song.tracks[2]; rack = track.devices[1]
        song.view.selected_track = track; track.view.selected_device = rack
        song.view.highlighted_clip_slot = track.clip_slots[1]; song.view.detail_clip = song.tracks[1].clip_slots[2].clip
        song.view.selected_parameter = rack.chains[0].devices[0].parameters[0]
        events = self.events(subscription, "selection")
        self.assertEqual(len(events), 1, "coalesced to the last selection")
        selection = mapper.snapshot()["selection"]
        self.assertEqual(events[-1]["payload"], {"track": selection["trackRef"], "scene": selection["sceneRef"], "clipSlot": selection["slotRef"], "detailClip": selection["detailClipRef"], "device": selection["deviceRef"], "parameter": selection["parameterRef"]})
        self.assertTrue(all(value is not None for value in events[-1]["payload"].values()), events[-1]["payload"])
        # The selected track's device changes: the listener moved to the newly selected track.
        track.view.selected_device = track.devices[0]
        self.assertEqual(self.events(subscription, "selection")[-1]["payload"]["device"], f"{mapper.refs.epoch}:device:2:0")
        song.tracks[0].view.selected_device = song.tracks[0].devices[1]
        self.assertEqual(self.events(subscription, "selection"), [], "another track's device selection isn't the selection")
        subscription.close()

    def test_names_colours_and_mixer_values_carry_their_objects_refs(self):
        song = listening_song(); mapper = LiveObjectMapper(song); subscription = _Subscription(mapper, {"name", "mixer"})
        epoch = mapper.refs.epoch
        song.tracks[2].name = "Bass"; song.scenes[1].color_index = 9; song.tracks[1].clip_slots[2].clip.name = "Hook"
        song.tracks[3].mute = True; song.tracks[3].mixer_device.volume.value = 0.8; song.tracks[3].mixer_device.volume.value = 0.9
        song.tracks[3].mixer_device.sends[0].value = 0.1; song.tracks[3].mixer_device.sends[1].value = 0.2; song.master_track.mixer_device.panning.value = 0.25
        observed = [(event["type"], event["ref"], event["payload"]) for event in self.events(subscription)]
        self.assertEqual(observed, [
            ("name", f"{epoch}:track:2", {"field": "name", "value": "Bass"}),
            ("name", f"{epoch}:scene:1", {"field": "color", "value": 9}),
            ("name", f"{epoch}:clip:1:2", {"field": "name", "value": "Hook"}),
            ("mixer", f"{epoch}:track:3", {"field": "mute", "value": True}),
            ("mixer", f"{epoch}:track:3", {"field": "volume", "value": 0.9}),
            ("mixer", f"{epoch}:track:3", {"field": "send", "index": 0, "value": 0.1}),
            ("mixer", f"{epoch}:track:3", {"field": "send", "index": 1, "value": 0.2}),
            ("mixer", f"{epoch}:track:4", {"field": "panning", "value": 0.25}),
        ])
        snapshot = mapper.snapshot()
        self.assertEqual({row["ref"] for row in snapshot["tracks"]} >= {f"{epoch}:track:2", f"{epoch}:track:3", f"{epoch}:track:4"}, True)
        subscription.close()

    def test_the_selected_devices_parameters_report_their_values_and_follow_the_selection(self):
        song = listening_song(); mapper = LiveObjectMapper(song); subscription = _Subscription(mapper, {"parameter"})
        first = song.tracks[0].devices[0]
        first.parameters[1].value = 0.75
        events = self.events(subscription, "parameter")
        self.assertEqual(events, [{"epoch": mapper.refs.epoch, "type": "parameter", "payload": {"value": 0.75}, "ref": mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][1]["ref"], "sequence": events[0]["sequence"]}])
        song.view.selected_track = song.tracks[2]; subscription.refresh()
        first.parameters[1].value = 0.25
        self.assertEqual(self.events(subscription, "parameter"), [], "the previous device is no longer followed")
        song.tracks[2].devices[0].parameters[0].value = 0.5
        self.assertEqual([event["ref"] for event in self.events(subscription, "parameter")], [mapper.snapshot()["tracks"][2]["devices"][0]["parameters"][0]["ref"]])
        subscription.close()

    def test_structure_changes_report_and_reattach_by_position(self):
        song = listening_song(); mapper = LiveObjectMapper(song); subscription = _Subscription(mapper, {"structure", "name"})
        epoch = mapper.refs.epoch
        song.tracks[0].clip_slots[1].clip = ListenClip(2.0)
        added = ListenTrack(); added.name = "New"; added.clip_slots = [ListenSlot() for _ in range(3)]; added.devices = []; added.mixer_device = ListenMixer(); added.view = ListenTrackView()
        old_first = song.tracks[0]
        song.tracks = [added] + song.tracks
        song.tracks[3].notify("devices"); song.cue_points = [FakeLocator(4.0)]
        observed = [(event["payload"], event.get("ref")) for event in self.events(subscription, "structure")]
        self.assertEqual(observed, [({"what": "clips"}, f"{epoch}:clip_slot:0:1"), ({"what": "tracks"}, None), ({"what": "devices"}, f"{epoch}:track:3"), ({"what": "locators"}, None)])
        # Re-attached by position: the old first track is track 1 now, and the new clip reports its name.
        old_first.name = "Moved"; song.tracks[1].clip_slots[1].clip.name = "Fresh"; added.name = "Renamed"
        self.assertEqual([(event["ref"], event["payload"]["value"]) for event in self.events(subscription, "name")], [(f"{epoch}:track:1", "Moved"), (f"{epoch}:clip:1:1", "Fresh"), (f"{epoch}:track:0", "Renamed")])
        song.tracks = song.tracks[1:]; subscription.refresh()
        self.assertEqual(added.listening(), 0, "a removed track's listeners are detached")
        subscription.close()

    def test_a_big_sets_listeners_attach_within_the_ticks_budget(self):
        song = listening_song(tracks=30); mapper = LiveObjectMapper(song)
        subscription = _Subscription(mapper, {"name"}, deadline=time.monotonic() - 1)
        self.assertTrue(all(track.listening() == 0 for track in song.tracks), "nothing past the budget")
        subscription.refresh()
        self.assertTrue(all(track.listening() == 2 for track in song.tracks))
        subscription.close()


class ShapeProbeTests(unittest.TestCase):
    """Review: what a Live shape offers is probed on representatives, never by walking the Set, so a
    big Set (or one long warped recording) can't slow or break status."""

    def test_status_reads_only_the_sets_first_tracks(self):
        class Marker:
            def __init__(self, value): self.beat_time = value; self.sample_time = value * 100.0
        class Unreadable(FakeClip):
            @property
            def warp_markers(self): raise RuntimeError("never read to advertise")
        song = FakeSong(); song.tracks = [FakeTrack() for _ in range(200)]
        long_take = FakeClip(4.0); long_take.is_audio_clip = True; long_take.warp_markers = [Marker(float(index)) for index in range(3000)]; song.tracks[0].clip_slots[0].clip = long_take
        broken = Unreadable(4.0); broken.is_audio_clip = True; song.tracks[150].clip_slots[0].clip = broken
        counter = ReadCounter(song.tracks)
        status = LiveObjectMapper(song).status()
        self.assertIn("clip.create", status["operations"]); self.assertIn("device.parameter.set", status["operations"])
        self.assertTrue(set(counter.reads) <= set(range(LiveObjectMapper._PROBE_SAMPLE)), f"only the first tracks are looked at: {sorted(counter.reads)}")

    def test_inside_live_its_classes_answer_and_no_track_is_read(self):
        def module(**classes): return types.SimpleNamespace(**classes)
        class Clip:
            def add_new_notes(self, notes): pass
            def apply_note_modifications(self, notes): pass
            def get_notes_extended(self, *args): pass
            def remove_notes_by_id(self, ids): pass
            def create_automation_envelope(self, parameter): pass
            def crop(self): pass
            name = property(lambda self: ""); muted = property(lambda self: False); view = property(lambda self: None)
        class ClipSlot:
            def fire(self): pass
            def create_clip(self, length): pass
            def delete_clip(self): pass
            def duplicate_clip_to(self, slot): pass
        class Track:
            def stop_all_clips(self): pass
            def insert_device(self, name, index): pass
            def delete_device(self, index): pass
            mixer_device = property(lambda self: None); color_index = property(lambda self: 0); arm = property(lambda self: False); name = property(lambda self: "")
        class DeviceParameter:
            value = property(lambda self: 0.0)
            def re_enable_automation(self): pass
        class Device:
            parameters = property(lambda self: []); name = property(lambda self: ""); view = property(lambda self: None)
        live = types.ModuleType("Live")
        live.Clip = module(Clip=Clip); live.ClipSlot = module(ClipSlot=ClipSlot); live.Track = module(Track=Track); live.DeviceParameter = module(DeviceParameter=DeviceParameter); live.Device = module(Device=Device)
        live.Song = module(Song=type("Song", (), {}))  # Live always has its Song class: that's what says this is Live
        song = FakeSong(); song.tracks = [FakeTrack() for _ in range(50)]; counter = ReadCounter(song.tracks)
        with patch.dict(sys.modules, {"Live": live}):
            operations = set(LiveObjectMapper(song).status()["operations"])
        self.assertEqual(counter.reads, {}, "Live's classes answer; the Set isn't read")
        self.assertTrue({"clip.create", "clip.delete", "clip.move", "note.add-batch", "note.update", "note.delete", "session.clip-launch", "session.clip-stop", "device.insert", "device.delete", "device.parameter.set", "parameter.re-enable-automation", "automation.envelope.create", "clip.action", "clip.set", "mixer.set", "track.set", "clip.rename", "device.rename"} <= operations, operations)
        self.assertNotIn("note.read-by-id", operations, "a member Live's class doesn't have isn't offered")

    def test_a_stand_in_live_module_without_songs_class_is_not_live(self):
        # A harness fakes a few of Live's modules (its browser): the Set's own objects answer.
        live = types.ModuleType("Live"); live.Application = types.SimpleNamespace(Application=object)
        with patch.dict(sys.modules, {"Live": live}):
            stand_in = set(LiveObjectMapper(FakeSong()).status()["operations"])
        # The same as with no Live module at all (but the module's own audit, which needs only a module).
        self.assertEqual(stand_in - {"dev.lom-audit"}, set(LiveObjectMapper(FakeSong()).status()["operations"]))
        self.assertIn("track.rename", stand_in)


class OldBoundTests(unittest.TestCase):
    """Review: every formerly capped argument, one past its old literal bound, passes the registry and
    reaches the operation's own fences, so the Remote Script and the registry stay in step."""

    def test_playback_targets_and_scene_indices_past_their_old_bounds_reach_the_fences(self):
        mapper = LiveObjectMapper(FakeSong()); snapshot = mapper.snapshot(); scene = snapshot["scenes"][0]; epoch = mapper.refs.epoch
        targets = [f"{epoch}:track:0|{epoch}:clip_slot:0:{index}|{scene['ref']}" for index in range(257)]
        cases = [
            ("session.emergency-stop", {"expectedTargets": targets, "expectedRecording": "stopped"}, "active playback does not exactly match"),
            ("session.audition-launch", {"ref": scene["ref"], "setName": "Set", "sceneName": "Scene 1", "sceneIndex": 10001, "playbackRevision": "revision", "eligibleTargets": targets, "outputSafety": {"safe": True, "provenance": "unit-test"}, "expectedSetIdentity": "live:set", "expectedAuthorityRevision": "0" * 64}, "Set identity does not match"),
            ("session.audition-stop", {"ref": scene["ref"], "setName": "Set", "eligibleTargets": targets, "expectedSetIdentity": "live:set", "expectedAuthorityRevision": "0" * 64}, "Set identity does not match"),
        ]
        for operation, args, fence in cases:
            validate_operation_payload(operation, "request", args)
            with self.assertRaisesRegex(ValueError, fence): mapper.invoke(operation, args)

    def test_a_note_duplication_of_hundreds_of_notes(self):
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live"); transaction = "old-bound-notes"
        created = mapper.invoke("clip.create", ControlSurfaceTests.clip_creation_args(mapper, mapper.snapshot()["tracks"][0]["ref"], 0, kind="midi", name="Dense", length=2048), transaction)
        added = mapper.invoke("note.add-batch", {"ref": created["ref"], "notes": [{"pitch": 60, "start": index * 0.5, "duration": 0.25, "velocity": 100, "channel": 1} for index in range(600)], **ControlSurfaceTests.note_authority(mapper, created["ref"])}, transaction)
        clip = song.tracks[0].clip_slots[0].clip
        clip.duplicate_notes_by_id = lambda ids: clip.add_new_notes([{key: value for key, value in note.items() if key != "note_id"} for note in list(clip.notes) if note["note_id"] in set(ids)])
        request = {"ref": created["ref"], "noteIds": added["noteIds"][:513], **ControlSurfaceTests.note_authority(mapper, created["ref"])}
        validate_operation_payload("note.duplicate", "request", request)
        self.assertEqual(mapper.invoke("note.duplicate", request, transaction)["duplicated"], 513)

    def test_an_envelope_clear_over_hundreds_of_parameters(self):
        song = FakeSong(); device = song.tracks[0].devices[0]; device.parameters = [FakeParameter() for _ in range(600)]
        clip = FakeClip(4.0); clip.clear_all_envelopes = lambda: None; clip.automation_envelope = lambda parameter: None; song.tracks[0].clip_slots[0].clip = clip
        mapper = LiveObjectMapper(song); clip_ref = mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        request = {"clipRef": clip_ref, "expectedAuthorityDigest": mapper._clip_authority_digest(clip_ref), "expectedEnvelopesRevision": hashlib.sha256(mapper._bounded_canonical([False] * 600).encode("utf-8")).hexdigest()}
        validate_operation_payload("automation.envelope.clear", "request", request)
        self.assertEqual(mapper.invoke("automation.envelope.clear", request)["cleared"], 0)

    def test_a_midi_capture_of_three_hundred_clips(self):
        song = FakeSong(); song.tracks = [FakeTrack() for _ in range(300)]
        def capture():
            for track in song.tracks: track.clip_slots[0].clip = FakeClip(4.0)
        song.capture_midi = capture
        mapper = LiveObjectMapper(song, provenance="real-live")
        result = mapper.invoke("session.capture-midi", {"expectedStateRevision": mapper._capture_authority_revision()}, "old-bound-capture")
        self.assertEqual(len(result["clips"]), 300); validate_operation_payload("session.capture-midi", "result", result)


class ScaleBenchmarkTests(unittest.TestCase):
    """A synthetic big Set: 200 tracks, 1000 devices (four with 5000 parameters), a clip of 20000
    notes. Timings are printed, not asserted (CI machines vary); what's asserted is what each
    targeted path reads: nothing below any track but its own."""

    @staticmethod
    def big_set():
        song = FakeSong(); song.scenes = [FakeScene(f"S{index}") for index in range(8)]; tracks = []
        for index in range(200):
            track = FakeTrack(); track.name = f"Track {index}"; track.clip_slots = [FakeSlot() for _ in range(8)]; track.mixer_device = FakeMixerDevice(); track.devices = []
            for position in range(5):
                device = FakeDevice(); device.name = f"Device {index}.{position}"
                heavy = position == 2 and index in (50, 100, 150, 199)
                device.parameters = [FakeParameter() for _ in range(5000 if heavy else 1)]
                track.devices.append(device)
            tracks.append(track)
        dense = FakeClip(40000.0); dense.notes = [{"pitch": 36 + note % 60, "start_time": note * 2.0, "duration": 0.5, "velocity": 100, "note_id": note + 1} for note in range(20000)]
        tracks[100].clip_slots[0].clip = dense
        song.tracks = tracks
        return song

    def test_targeted_reads_on_a_big_set_touch_only_their_track(self):
        song = self.big_set(); mapper = LiveObjectMapper(song); timings = {}
        started = time.perf_counter(); whole = mapper.snapshot(); timings["full snapshot"] = time.perf_counter() - started
        self.assertEqual((whole["trackCount"], sum(len(row["devices"]) for row in whole["tracks"]), len(whole["tracks"][100]["clips"][0]["notes"])), (200, 1000, 20000))
        parameter = whole["tracks"][150]["devices"][2]["parameters"][4999]
        args = {"ref": parameter["ref"], "value": 0.75, "expectedRevision": parameter["revision"], **ControlSurfaceTests.parameter_authority(mapper, parameter["ref"])}
        counter = ReadCounter(song.tracks)
        def measure(name, work, own):
            counter.reads.clear(); started = time.perf_counter(); result = work(); timings[name] = time.perf_counter() - started
            self.assertEqual(set(counter.reads) - own, set(), f"{name} read other tracks: {sorted(set(counter.reads) - own)[:10]}")
            return result
        focused = measure("focused snapshot (1 of 200 tracks)", lambda: mapper.snapshot({"focus": [100], "parts": ["tracks"]}), {100})
        self.assertEqual(len(focused["tracks"][100]["clips"][0]["notes"]), 20000); self.assertTrue(focused["tracks"][0]["light"])
        measure("light snapshot (every track, none walked)", lambda: mapper.snapshot({"focus": [], "parts": ["tracks"]}), set())
        measure("get(parameter of a 5000-parameter device)", lambda: mapper.get(parameter["ref"]), {150})
        measure("get(track)", lambda: mapper.get(whole["tracks"][7]["ref"]), {7})
        measure("authority digest (device.parameter.set)", lambda: _authority_state_digest(mapper, args, "device.parameter.set"), {150})
        measure("structure revision", mapper._structure_revision, set())
        measure("playback", mapper._playback, set())
        changed = measure("device.parameter.set", lambda: mapper.invoke("device.parameter.set", args), {150})
        self.assertEqual(changed["value"], 0.75)
        print("\n  scale benchmark (200 tracks, 1000 devices, 4 x 5000 parameters, 20000 notes):")
        for name, seconds in timings.items(): print(f"    {name:48s} {seconds * 1000:9.1f} ms")


def mutate_through(bridge, operation, args, key, transaction="transaction-phase2", holder=None):
    """A single-request change as the host sends it: the preview's digest, then `mutate`, both
    checked against the operation's registry schema."""
    validate_operation_payload(operation, "request", args)
    digest = bridge.mapper.invoke("authority.digest", {"operation": operation, "args": args})["stateDigest"]
    result = bridge._dispatch_with_holder("mutate", {"operation": operation, "transactionId": transaction, "idempotencyKey": key, "stateDigest": digest, "args": args}, {} if holder is None else holder)
    validate_operation_payload(operation, "result", result)
    return result


def read_through(bridge, operation, args):
    """A read as the host sends it: invoked with no mutation authority, checked against the registry."""
    validate_operation_payload(operation, "request", args)
    result = bridge._dispatch_with_holder("invoke", {"operation": operation, "args": args}, {})
    validate_operation_payload(operation, "result", result)
    return result


class FakeDataTrack(FakeTrack):
    def __init__(self):
        super().__init__(); self.data = {}
    def get_data(self, key, default): return self.data.get(key, default)
    def set_data(self, key, value): self.data[key] = value


class FakeDataSong(FakeSong):
    def __init__(self):
        super().__init__(); self.data = {}; self.tracks = [FakeDataTrack(), FakeDataTrack()]
    def get_data(self, key, default): return self.data.get(key, default)
    def set_data(self, key, value): self.data[key] = value


class SetDataTests(unittest.TestCase):
    """data.get/data.set: text saved inside the Set (Song.get_data/set_data) or with a track."""

    def test_text_saved_in_the_set_and_on_a_track_reads_back(self):
        song = FakeDataSong(); bridge = immediate_bridge(song); snapshot = bridge.mapper.snapshot()
        set_ref, track_ref = snapshot["set"]["ref"], snapshot["tracks"][1]["ref"]
        self.assertEqual({operation for operation in ("data.get", "data.set") if not remote_module._mutation_authority_required(operation)}, {"data.get"})
        self.assertEqual(read_through(bridge, "data.get", {"ref": set_ref, "key": "kumi.notes"}), {"ref": set_ref, "key": "kumi.notes", "value": None})
        self.assertEqual(mutate_through(bridge, "data.set", {"ref": set_ref, "key": "kumi.notes", "value": "verse at bar 9"}, "data-key-0001"), {"ref": set_ref, "key": "kumi.notes", "value": "verse at bar 9", "prior": None})
        self.assertEqual(song.data, {"kumi.notes": "verse at bar 9"})
        # A track keeps its own: the Set's is untouched.
        self.assertEqual(mutate_through(bridge, "data.set", {"ref": track_ref, "key": "kumi.notes", "value": "bass"}, "data-key-0002")["prior"], None)
        self.assertEqual((song.tracks[1].data, song.tracks[0].data, song.data["kumi.notes"]), ({"kumi.notes": "bass"}, {}, "verse at bar 9"))
        self.assertEqual(read_through(bridge, "data.get", {"ref": track_ref, "key": "kumi.notes"})["value"], "bass")
        # Compare-and-set: only while the key holds what was read.
        with self.assertRaisesRegex(ValueError, "changed since it was read"): bridge.mapper.invoke("data.set", {"ref": set_ref, "key": "kumi.notes", "value": "x", "expectedValue": "chorus"})
        self.assertEqual(mutate_through(bridge, "data.set", {"ref": set_ref, "key": "kumi.notes", "value": None, "expectedValue": "verse at bar 9"}, "data-key-0003"), {"ref": set_ref, "key": "kumi.notes", "value": None, "prior": "verse at bar 9"})

    def test_what_isnt_text_or_isnt_there_is_refused_and_an_unconfirmed_write_goes_back(self):
        song = FakeDataSong(); mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); set_ref = snapshot["set"]["ref"]
        song.data["kumi.odd"] = {"not": "text"}
        with self.assertRaisesRegex(ValueError, "isn't text"): mapper.invoke("data.get", {"ref": set_ref, "key": "kumi.odd"})
        with self.assertRaisesRegex(ValueError, "isn't text"): mapper.invoke("data.set", {"ref": set_ref, "key": "kumi.odd", "value": "mine"})
        self.assertEqual(song.data["kumi.odd"], {"not": "text"})
        # Another control surface's data in the Set can be read, never written or cleared.
        song.data["other.script"] = "theirs"
        self.assertEqual(mapper.invoke("data.get", {"ref": set_ref, "key": "other.script"})["value"], "theirs")
        for value in ("mine", None):
            with self.assertRaisesRegex(ValueError, "Kumi writes only its own keys"): mapper.invoke("data.set", {"ref": set_ref, "key": "other.script", "value": value})
        self.assertEqual(song.data["other.script"], "theirs")
        with self.assertRaisesRegex(ValueError, "track reference is stale or invalid"): mapper.invoke("data.get", {"ref": f"{mapper.refs.epoch}:track:9", "key": "k"})
        with self.assertRaisesRegex(ValueError, "track reference is stale or invalid"): mapper.invoke("data.get", {"ref": "0:track:0", "key": "k"})
        # Live keeps something else than was written: the prior value goes back and the change fails.
        song.data["kumi.k"] = "before"; song.set_data = lambda key, value: song.data.__setitem__(key, value if value == "before" else value.upper())
        with self.assertRaisesRegex(ValueError, "^data change was not confirmed$"): mapper.invoke("data.set", {"ref": set_ref, "key": "kumi.k", "value": "after"})
        self.assertEqual(song.data["kumi.k"], "before")
        self.assertTrue(mapper._operation_supported("data.set")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("data.get"))


class FakeSelectableNoteClip(FakeNoteClip):
    """Live 12's note selection and region deletion on top of FakeNoteClip."""

    def __init__(self, length=4.0, notes=()):
        super().__init__(length, notes); self.selected = set(); self.extra_removal = None
    def select_all_notes(self): self.selected = set(self.stored)
    def deselect_all_notes(self): self.selected = set()
    def select_notes_by_id(self, ids): self.selected |= set(ids)
    def get_selected_notes_extended(self): return FakeMidiNoteVector(self.stored[note_id].copy() for note_id in sorted(self.selected))
    def remove_notes_extended(self, from_pitch, pitch_span, from_time, time_span):
        for note_id, note in list(self.stored.items()):
            if from_pitch <= note.pitch < from_pitch + pitch_span and from_time <= note.start_time < from_time + time_span: del self.stored[note_id]
        if self.extra_removal is not None: self.stored.pop(self.extra_removal, None)
    def add_new_notes(self, notes):
        for note in notes:
            value = note if isinstance(note, dict) else {"pitch": note.pitch, "start_time": note.start_time, "duration": note.duration, "velocity": note.velocity}
            note_id = max(self.stored, default=0) + 1
            self.stored[note_id] = FakeMidiNote(note_id, value["pitch"], value["start_time"], value["duration"], value["velocity"], value.get("mute", False), value.get("probability", 1.0), value.get("velocity_deviation", 0.0), value.get("release_velocity", 64.0))


class NoteSelectionAndRegionTests(unittest.TestCase):
    """note.select and note.delete-range, checked on the clip's hierarchy (and its notes, to delete)."""

    def clip_bridge(self):
        clip = FakeSelectableNoteClip(4.0, [FakeMidiNote(1, 60, 0.0, 0.5), FakeMidiNote(2, 62, 1.0, 0.5), FakeMidiNote(3, 64, 2.0, 0.5), FakeMidiNote(4, 72, 1.0, 0.5)])
        song = FakeSong(); song.tracks[0].clip_slots[0].clip = clip; bridge = immediate_bridge(song)
        row = bridge.mapper.snapshot()["tracks"][0]["clips"][0]
        return bridge, clip, row["ref"]

    def test_notes_are_selected_all_none_or_exactly_by_id(self):
        bridge, clip, reference = self.clip_bridge(); authority = bridge.mapper._session_clip_authority(reference)
        self.assertEqual(mutate_through(bridge, "note.select", {"ref": reference, "all": True, "expectedClipAuthority": authority}, "select-key-0001"), {"selected": 4})
        self.assertEqual(mutate_through(bridge, "note.select", {"ref": reference, "noteIds": [1, 3], "expectedClipAuthority": authority}, "select-key-0002"), {"selected": 2})
        self.assertEqual(clip.selected, {1, 3})
        self.assertEqual(mutate_through(bridge, "note.select", {"ref": reference, "none": True, "expectedClipAuthority": authority}, "select-key-0003"), {"selected": 0})
        for broken, message in (({"all": True, "none": True}, "exactly one"), ({"none": False}, "exactly one"), ({"noteIds": [9]}, "not present in the clip"), ({"noteIds": [1, 1]}, "note ids are invalid")):
            with self.assertRaisesRegex(ValueError, message): bridge.mapper.invoke("note.select", {"ref": reference, "expectedClipAuthority": authority, **broken})
        # Another clip in the slot: the preview's hierarchy no longer holds.
        bridge.mapper.song.tracks[0].clip_slots[0].clip = FakeSelectableNoteClip(4.0, [FakeMidiNote(1, 60, 0.0, 0.5)])
        with self.assertRaisesRegex(ValueError, "hierarchy identity changed"): bridge.mapper.invoke("note.select", {"ref": reference, "all": True, "expectedClipAuthority": authority})
        self.assertTrue(bridge.mapper._operation_supported("note.select")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("note.select"))

    def test_a_region_loses_exactly_its_notes_or_nothing(self):
        bridge, clip, reference = self.clip_bridge()
        def request(**fields):
            notes = bridge.mapper._read_notes(clip)
            return {"ref": reference, "fromPitch": 60, "pitchSpan": 6, "fromTime": 0.5, "timeSpan": 2.0, "expectedClipAuthority": bridge.mapper._session_clip_authority(reference), "expectedNotesRevision": hashlib.sha256(bridge.mapper._bounded_canonical(notes).encode()).hexdigest(), **fields}
        # Pitches 60-65 starting in [0.5, 2.5): notes 2 and 3; note 1 starts before, note 4 is above.
        result = mutate_through(bridge, "note.delete-range", request(), "range-key-0001")
        self.assertEqual(result["deleted"], 2); self.assertEqual(sorted(clip.stored), [1, 4])
        self.assertEqual(result["notesRevision"], hashlib.sha256(bridge.mapper._bounded_canonical(bridge.mapper._read_notes(clip)).encode()).hexdigest())
        # Live takes a note outside the region too: the notes taken go back and it fails.
        clip.stored[5] = FakeMidiNote(5, 61, 1.5, 0.5); clip.extra_removal = 4
        before = sorted((note.pitch, note.start_time) for note in clip.stored.values())
        with self.assertRaisesRegex(ValueError, "changed notes outside its region"): bridge.mapper.invoke("note.delete-range", request())
        self.assertEqual(sorted((note.pitch, note.start_time) for note in clip.stored.values()), before)
        with self.assertRaisesRegex(ValueError, "clip notes changed since preview"): bridge.mapper.invoke("note.delete-range", request(expectedNotesRevision="0" * 64))
        with self.assertRaisesRegex(ValueError, "the time range is invalid"): bridge.mapper.invoke("note.delete-range", request(timeSpan=0))
        self.assertTrue(bridge.mapper._operation_supported("note.delete-range"))


class FakeFireSlot(FakeSlot):
    def __init__(self):
        super().__init__(); self.presses = []
    def set_fire_button_state(self, pressed): self.presses.append(pressed)


class FakeFireScene(FakeScene):
    def __init__(self, name="Scene 1"):
        super().__init__(name); self.presses = []
    def set_fire_button_state(self, pressed): self.presses.append(pressed)


class FakeFireClip(FakeClip):
    def __init__(self, length=4.0):
        super().__init__(length); self.presses = []
    def set_fire_button_state(self, pressed): self.presses.append(pressed)


def fire_song():
    song = FakeSong(); slot = FakeFireSlot(); slot.clip = FakeFireClip(); song.tracks[0].clip_slots = [slot, FakeFireSlot()]; song.scenes = [FakeFireScene(), FakeFireScene("Scene 2")]
    return song


class Closable:
    def close(self): pass


class PlayingControlTests(unittest.TestCase):
    """fire-button.set held per connection, track.action and transport.action jump-by."""

    SAFE = {"safe": True, "provenance": "test-harness"}

    def press(self, row, pressed):
        return {"ref": row["ref"], "pressed": pressed, "expectedObjectIdentity": row["objectIdentity"], "outputSafety": self.SAFE}

    def test_a_press_is_held_for_its_connection_and_every_guard_lets_it_go(self):
        song = fire_song(); bridge = immediate_bridge(song); bridge._connections = []; bridge._clients = set(); snapshot = bridge.mapper.snapshot(); holder = {}
        clip_row, slot_row, scene_row = snapshot["tracks"][0]["clips"][0], snapshot["tracks"][0]["clipSlots"][1], snapshot["scenes"][1]
        clip, slot, scene = song.tracks[0].clip_slots[0].clip, song.tracks[0].clip_slots[1], song.scenes[1]
        self.assertEqual(mutate_through(bridge, "fire-button.set", self.press(clip_row, True), "fire-key-0001", holder=holder), {"ref": clip_row["ref"], "pressed": True})
        self.assertEqual((clip.presses, list(bridge.mapper._held_fire_buttons)), ([True], [clip_row["ref"]]))
        # The connection lets go itself.
        mutate_through(bridge, "fire-button.set", self.press(clip_row, False), "fire-key-0002", holder=holder)
        self.assertEqual((clip.presses, bridge.mapper._held_fire_buttons), ([True, False], {}))
        # Its connection closing lets go of what it holds; another connection closing doesn't.
        mutate_through(bridge, "fire-button.set", self.press(scene_row, True), "fire-key-0003", holder=holder)
        bridge._close(types.SimpleNamespace(holder={}, socket=Closable())); self.assertEqual(scene.presses, [True])
        bridge._close(types.SimpleNamespace(holder=holder, socket=Closable())); self.assertEqual(scene.presses, [True, False])
        # Held past its deadline, the display tick lets go.
        mutate_through(bridge, "fire-button.set", self.press(slot_row, True), "fire-key-0004", holder=holder)
        expires = bridge.mapper._held_fire_buttons[slot_row["ref"]]["expiresAt"]; self.assertAlmostEqual(expires - time.time() * 1000, 30000, delta=2000)
        with patch("ableton_mcp_remote_script.time.time", return_value=(expires - 1) / 1000): bridge.mapper.fire_button_tick()
        self.assertEqual(slot.presses, [True])
        with patch("ableton_mcp_remote_script.time.time", return_value=expires / 1000): bridge.mapper.fire_button_tick()
        self.assertEqual(slot.presses, [True, False])
        # A press needs the output-safety evidence launches need, and the target it previewed.
        with self.assertRaisesRegex(ValueError, "output-safety"): bridge.mapper.invoke("fire-button.set", {**self.press(clip_row, True), "outputSafety": {"safe": True, "provenance": "unknown"}})
        with self.assertRaisesRegex(ValueError, "target changed since preview"): bridge.mapper.invoke("fire-button.set", {**self.press(clip_row, True), "expectedObjectIdentity": scene_row["objectIdentity"]})
        with self.assertRaisesRegex(ValueError, "holds no clip"): bridge.mapper.invoke("fire-button.set", self.press({**clip_row, "ref": clip_row["ref"].rsplit(":", 1)[0] + ":1"}, True))
        # A reconnect lets go of everything held.
        mutate_through(bridge, "fire-button.set", self.press(clip_row, True), "fire-key-0005", holder=holder)
        bridge.mapper.invoke("session.reconnect", {}); self.assertEqual((clip.presses[-1], bridge.mapper._held_fire_buttons), (False, {}))
        self.assertTrue(bridge.mapper._operation_supported("fire-button.set")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("fire-button.set"))

    def test_shutting_the_bridge_down_lets_go_of_pressed_fire_buttons(self):
        import socket as _socket
        probe = _socket.socket(); probe.bind(("127.0.0.1", 0)); port = probe.getsockname()[1]; probe.close()
        instance = FakeInstance(); instance.song = fire_song(); live = AbletonMcpBridge(instance, {"host": "127.0.0.1", "port": port, "secret": "x" * 40})
        row = live.mapper.snapshot()["scenes"][0]
        live.mapper.invoke("fire-button.set", self.press(row, True)); live.update_display(); self.assertEqual(instance.song.scenes[0].presses, [True])
        live.disconnect(); self.assertEqual(instance.song.scenes[0].presses, [True, False])

    def test_a_running_clip_is_jumped_in_and_the_playhead_jumps_by_beats(self):
        song = FakeSong(); track = song.tracks[0]; track.jumps = []; track.jump_in_running_session_clip = lambda beats: track.jumps.append(beats)
        song.jumps = []; song.jump_by = lambda beats: song.jumps.append(beats)
        bridge = immediate_bridge(song); snapshot = bridge.mapper.snapshot(); row = snapshot["tracks"][0]
        args = {"ref": row["ref"], "action": "jump-in-running-clip", "beats": 4, "expectedObjectIdentity": row["objectIdentity"]}
        with self.assertRaisesRegex(ValueError, "no Session clip is playing"): bridge.mapper.invoke("track.action", args)
        track.playing_slot_index = 0
        self.assertEqual(mutate_through(bridge, "track.action", args, "track-action-0001"), {"done": True}); self.assertEqual(track.jumps, [4.0])
        with self.assertRaisesRegex(ValueError, "beats is required"): bridge.mapper.invoke("track.action", {key: value for key, value in args.items() if key != "beats"})
        self.assertTrue(bridge.mapper._operation_supported("track.action")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("track.action"))
        transport = {"setRef": snapshot["set"]["ref"], "action": "jump-by", "beats": -8, "expectedObjectIdentity": snapshot["set"]["objectIdentity"], "expectedRevision": bridge.mapper._playback()["revision"]}
        self.assertTrue(mutate_through(bridge, "transport.action", transport, "jump-key-0001")["done"]); self.assertEqual(song.jumps, [-8.0])
        with self.assertRaisesRegex(ValueError, "beats is required for jump-by"): bridge.mapper.invoke("transport.action", {key: value for key, value in transport.items() if key != "beats"})


class FakeEnvelopeEvent:
    def __init__(self, time, value): self.time = time; self.value = value


class FakeStepEnvelope:
    """A clip envelope with Live's event API, insert_step and value_at_time (a held value per event)."""

    def __init__(self, clip, events=()):
        self.canonical_parent = clip; self.events = list(events); self.halve_steps = False
    def events_in_range(self, start, end): return [event for event in sorted(self.events, key=lambda event: event.time) if start <= event.time < end]
    def create_event(self, event): self.events.append(event)
    def delete_events_in_range(self, start, end): self.events = [event for event in self.events if not start <= event.time < end]
    def insert_step(self, start, length, value):
        if self.halve_steps: value = value / 2
        self.events = [event for event in self.events if not start <= event.time <= start + length] + [FakeEnvelopeEvent(start, value), FakeEnvelopeEvent(start + length - 1e-3, value)]
    def value_at_time(self, time):
        ordered = sorted(self.events, key=lambda event: event.time); before = [event for event in ordered if event.time <= time]
        return before[-1].value if before else (ordered[0].value if ordered else 0.0)


class FakeAutomationClip(FakeClip):
    def __init__(self, events=((0.0, 0.2),)):
        super().__init__(4.0); self.envelope = FakeStepEnvelope(self, [FakeEnvelopeEvent(time, value) for time, value in events])
    def automation_envelope(self, _parameter): return self.envelope
    def create_automation_envelope(self, _parameter): self.envelope = FakeStepEnvelope(self); return self.envelope
    def clear_envelope(self, _parameter): self.envelope = None


class AutomationStepTests(unittest.TestCase):
    """automation.step.insert (fenced as point inserts) and automation.value-at (a read)."""

    def test_a_step_holds_its_value_or_the_envelope_goes_back(self):
        song = FakeSong(); clip = FakeAutomationClip(); song.tracks[0].clip_slots[0].clip = clip; bridge = immediate_bridge(song); snapshot = bridge.mapper.snapshot()
        clip_ref, parameter_ref = snapshot["tracks"][0]["clips"][0]["ref"], snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]
        self.assertTrue(bridge.mapper._operation_supported("automation.step.insert") and bridge.mapper._operation_supported("automation.value-at"))
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("automation.value-at"))
        self.assertFalse(remote_module._mutation_authority_required("automation.value-at"))
        self.assertEqual(read_through(bridge, "automation.value-at", {"clipRef": clip_ref, "parameterRef": parameter_ref, "time": 1.5}), {"value": 0.2})
        def step(**fields):
            read = bridge.mapper.invoke("automation.envelope.read", {"clipRef": clip_ref, "parameterRef": parameter_ref})
            return {"clipRef": clip_ref, "parameterRef": parameter_ref, "start": 1.0, "length": 1.0, "value": 0.8, "expectedAuthorityDigest": bridge.mapper._envelope_authority_digest(clip_ref, parameter_ref), "expectedEnvelopeRevision": read["revision"], **fields}
        self.assertEqual(mutate_through(bridge, "automation.step.insert", step(), "step-key-0001"), {"inserted": 1})
        self.assertEqual(read_through(bridge, "automation.value-at", {"clipRef": clip_ref, "parameterRef": parameter_ref, "time": 1.5}), {"value": 0.8})
        # Live holds something else over the step: the envelope goes back exactly.
        before = [(event.time, event.value) for event in clip.envelope.events_in_range(0, 8)]; clip.envelope.halve_steps = True
        with self.assertRaisesRegex(ValueError, "^automation step was not confirmed$"): bridge.mapper.invoke("automation.step.insert", step(start=2.5, length=0.5, value=0.6))
        self.assertEqual([(event.time, event.value) for event in clip.envelope.events_in_range(0, 8)], before)
        with self.assertRaisesRegex(ValueError, "outside the clip"): bridge.mapper.invoke("automation.step.insert", step(start=3.5, length=1.0))
        with self.assertRaisesRegex(ValueError, "outside the parameter's range"): bridge.mapper.invoke("automation.step.insert", step(value=2.0))
        with self.assertRaisesRegex(ValueError, "envelope changed since preview"): bridge.mapper.invoke("automation.step.insert", step(expectedEnvelopeRevision="0" * 64))
        # No envelope yet: the value is null, and a step makes one.
        clip.envelope = None
        self.assertEqual(bridge.mapper.invoke("automation.value-at", {"clipRef": clip_ref, "parameterRef": parameter_ref, "time": 0.0}), {"value": None})
        self.assertEqual(bridge.mapper.invoke("automation.step.insert", step(start=0.0, length=2.0, value=0.4)), {"inserted": 1}); self.assertEqual(clip.envelope.value_at_time(1.0), 0.4)


def state_revision(state):
    """A fence as the host computes it: sha-256 of the canonical state."""
    return hashlib.sha256(LiveObjectMapper._bounded_canonical(state).encode()).hexdigest()


class FakeSample:
    def __init__(self):
        self.file_path = "/samples/break.wav"; self.length = 88200; self.sample_rate = 44100; self.warping = True; self.warp_mode = 0
        self.beats_granulation_resolution = 2; self.beats_transient_envelope = 100.0; self.beats_transient_loop_mode = 1; self.complex_pro_envelope = 128.0
        self.complex_pro_formants = 100.0; self.texture_flux = 0.0; self.texture_grain_size = 50.0; self.tones_grain_size = 30.0
        self.slicing_style = 0; self.slicing_beat_division = 4; self.slicing_region_count = 8; self.slicing_sensitivity = 0.5
        self.slices = [0, 22050, 44100]; self.ignore_inserts = False
    def insert_slice(self, time):
        if not self.ignore_inserts: self.slices = sorted(self.slices + [time])
    def move_slice(self, old, new): self.slices = sorted(new if value == old else value for value in self.slices)
    def remove_slice(self, time): self.slices = [value for value in self.slices if value != time]
    def clear_slices(self): self.slices = []
    def reset_slices(self): self.slices = [0, 44100]


class FakeSimpler(FakeDevice):
    """A Simpler as a Set lists it: Live's class_name for it is OriginalSimpler."""

    def __init__(self):
        super().__init__(); self.name = "Simpler"; self.class_name = "OriginalSimpler"; self.sample = FakeSample(); self.warps = []
        self.playback_mode = 0; self.retrigger = False; self.slicing_playback_mode = 1; self.voices = 8; self.pad_slicing = False; self.note_pitch_bend_range = 5
        self.multi_sample_mode = False; self.pitch_bend_range = 5; self.can_warp_as = True; self.can_warp_double = True; self.can_warp_half = False
    def warp_as(self, beats): self.warps.append(("as", beats))
    def warp_double(self): self.warps.append(("double",))
    def warp_half(self): self.warps.append(("half",))


class FakeRoar(FakeDevice):
    def __init__(self):
        super().__init__(); self.name = "Roar"; self.class_name = "Roar"; self.routing_mode_index = 0; self.routing_mode_list = ["Single", "Serial", "Parallel", "Multi Band", "Feedback"]; self.env_listen = False; self.clamp = None
    def __setattr__(self, name, value):
        if name == "routing_mode_index" and getattr(self, "clamp", None) is not None: value = min(value, self.clamp)
        object.__setattr__(self, name, value)


class WavetableDevice(FakeDevice):
    """Named as Live's own class: a Set's Wavetable reports class_name InstrumentVector."""

    def __init__(self):
        super().__init__(); self.name = "Wavetable"; self.class_name = "InstrumentVector"
        self.oscillator_1_wavetable_category = 0; self.oscillator_1_wavetable_index = 3; self.oscillator_2_wavetable_category = 1; self.oscillator_2_wavetable_index = 0
        self.oscillator_1_effect_mode = 0; self.oscillator_2_effect_mode = 0; self.filter_routing = 0; self.unison_mode = 0; self.unison_voice_count = 2; self.mono_poly = 1; self.poly_voices = 8
        self.oscillator_wavetable_categories = ["Basics", "Collection", "Complex"]; self.oscillator_1_wavetables = ["Sine", "Saw", "Square", "Pulse", "Formant", "Vox"]; self.oscillator_2_wavetables = ["Sine", "Saw"]
        self.visible_modulation_target_names = ["Osc 1 Pos", "Filter 1 Freq"]; self.amounts = {}
        cutoff = FakeParameter(); cutoff.name = "Filter 2 Freq"; self.parameters = [FakeParameter(), cutoff]
    def __setattr__(self, name, value):
        object.__setattr__(self, name, value)
        # Like Live: a new category lists other wavetables, so the index starts over.
        if name == "oscillator_1_wavetable_category": object.__setattr__(self, "oscillator_1_wavetable_index", 0)
    def get_modulation_value(self, target, source):
        if not 0 <= target < len(self.visible_modulation_target_names): raise RuntimeError("no such target")
        return self.amounts.get((target, source), 0.0)
    def set_modulation_value(self, target, source, value): self.amounts[(target, source)] = value
    def get_modulation_target_parameter_name(self, index): return self.visible_modulation_target_names[index]
    def is_parameter_modulatable(self, parameter): return True
    def add_parameter_to_modulation_matrix(self, parameter):
        self.visible_modulation_target_names = self.visible_modulation_target_names + [parameter.name]; return len(self.visible_modulation_target_names) - 1


def family_song():
    song = FakeSong(); hybrid = FakeDevice(); hybrid.name = "Hybrid Reverb"; hybrid.class_name = "Hybrid"; hybrid.ir_time_shaping_on = False; hybrid.ir_attack_time = 0.0; hybrid.ir_decay_time = 60.0; hybrid.ir_size_factor = 1.0
    song.tracks[0].devices = [FakeSimpler(), FakeRoar(), WavetableDevice(), hybrid, FakeDevice()]
    return song


class DeviceFamilyTests(unittest.TestCase):
    """device.property.set, device.action, sample.set/slice and wavetable.set/modulation.set, with the
    rows they are fenced on."""

    def test_family_rows_name_each_setting_by_the_operations_names(self):
        rows = LiveObjectMapper(family_song()).snapshot()["tracks"][0]["devices"]
        simpler, roar, wavetable, hybrid, utility = rows
        self.assertEqual(simpler["simpler"], {"playbackMode": 0, "retrigger": False, "slicingPlaybackMode": 1, "voices": 8, "padSlicing": False, "notePitchBendRange": 5, "multiSampleMode": False, "pitchBendRange": 5, "canWarpAs": True, "canWarpDouble": True, "canWarpHalf": False})
        self.assertEqual((simpler["sample"]["filePath"], simpler["sample"]["slices"], simpler["sample"]["slicingSensitivity"], simpler["sample"]["beatsGranulationResolution"]), ("/samples/break.wav", [0, 22050, 44100], 0.5, 2))
        self.assertEqual(roar["roar"], {"routingModeIndex": 0, "routingModeList": ["Single", "Serial", "Parallel", "Multi Band", "Feedback"], "envListen": False})
        self.assertEqual((wavetable["wavetable"]["oscillator1WavetableIndex"], wavetable["wavetable"]["categories"], wavetable["wavetable"]["visibleModulationTargetNames"]), (3, ["Basics", "Collection", "Complex"], ["Osc 1 Pos", "Filter 1 Freq"]))
        # Live calls Hybrid Reverb "Hybrid": its row is there, with the shaping switch.
        self.assertEqual((hybrid["hybridReverb"]["irTimeShapingOn"], hybrid["hybridReverb"]["decay"]), (False, 60.0))
        self.assertFalse({"simpler", "sample", "roar", "wavetable", "hybridReverb"} & set(utility))

    def test_a_device_setting_is_set_by_its_name_fenced_on_its_value(self):
        song = family_song(); bridge = immediate_bridge(song); rows = bridge.mapper.snapshot()["tracks"][0]["devices"]; roar_row, roar = rows[1], song.tracks[0].devices[1]
        def request(prop, value, row=roar_row, current=None):
            family, attribute, _ = LiveObjectMapper._DEVICE_PROPERTIES[prop]; shown = bridge.mapper.get(row["ref"])[LiveObjectMapper._FAMILY_ROW_KEYS[family]][LiveObjectMapper._camel(attribute)] if current is None else current
            return {"ref": row["ref"], "property": prop, "value": value, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": state_revision({"property": prop, "value": shown})}
        result = mutate_through(bridge, "device.property.set", request("roar.routing_mode_index", 2), "property-key-0001")
        self.assertEqual((result["changed"], result["value"], roar.routing_mode_index), (True, 2, 2))
        self.assertTrue(mutate_through(bridge, "device.property.set", request("roar.env_listen", True), "property-key-0002")["value"]); self.assertTrue(roar.env_listen)
        for prop, value, message in (("roar.routing_mode_index", 9, "not one of its choices"), ("roar.env_listen", 1, "takes true or false"), ("roar.routing_mode_index", 1.5, "whole number")):
            with self.assertRaisesRegex(ValueError, message): bridge.mapper.invoke("device.property.set", request(prop, value))
        with self.assertRaisesRegex(ValueError, "needs a Simpler; that device isn't one"): bridge.mapper.invoke("device.property.set", request("simpler.voices", 4, current=8))
        with self.assertRaisesRegex(ValueError, "device property state changed since preview"): bridge.mapper.invoke("device.property.set", request("roar.routing_mode_index", 1, current=0))
        # Live keeps another value: the prior one goes back.
        roar.clamp = 3
        with self.assertRaisesRegex(ValueError, "change was not confirmed"): bridge.mapper.invoke("device.property.set", request("roar.routing_mode_index", 4))
        self.assertEqual(roar.routing_mode_index, 2)
        simpler_row = rows[0]
        self.assertEqual(mutate_through(bridge, "device.property.set", request("simpler.playback_mode", 2, row=simpler_row), "property-key-0003")["value"], 2); self.assertEqual(song.tracks[0].devices[0].playback_mode, 2)
        self.assertTrue(bridge.mapper._operation_supported("device.property.set")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("device.property.set"))

    def test_simpler_actions_its_sample_settings_and_its_slices(self):
        song = family_song(); bridge = immediate_bridge(song); simpler = song.tracks[0].devices[0]; row = bridge.mapper.snapshot()["tracks"][0]["devices"][0]
        def fenced(state, **fields): return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": state_revision(state), **fields}
        sample_row = lambda: bridge.mapper.get(row["ref"])["sample"]
        self.assertEqual(mutate_through(bridge, "device.action", fenced({"sample": sample_row()}, action="simpler-warp-as", beats=8), "action-key-0001")["done"], True)
        self.assertEqual(simpler.warps, [("as", 8.0)])
        with self.assertRaisesRegex(ValueError, "isn't possible for this sample now"): bridge.mapper.invoke("device.action", fenced({"sample": sample_row()}, action="simpler-warp-half"))
        with self.assertRaisesRegex(ValueError, "beats goes with simpler-warp-as"): bridge.mapper.invoke("device.action", fenced({"sample": sample_row()}, action="simpler-warp-double", beats=2))
        with self.assertRaisesRegex(ValueError, "needs a CC Control"): bridge.mapper.invoke("device.action", fenced({"sample": sample_row()}, action="cc-control-resend"))
        # sample.set is fenced as the families are: every field it sets, as the sample row shows it.
        settings = lambda: {field: sample_row()[field] for field in LiveObjectMapper._SAMPLE_FIELDS}
        self.assertTrue(mutate_through(bridge, "sample.set", fenced(settings(), slicingRegionCount=16, slicingSensitivity=0.25), "sample-key-0001")["changed"])
        self.assertEqual((simpler.sample.slicing_region_count, type(simpler.sample.slicing_region_count), simpler.sample.slicing_sensitivity), (16, int, 0.25))
        with self.assertRaisesRegex(ValueError, "takes a whole number"): bridge.mapper.invoke("sample.set", fenced(settings(), slicingRegionCount=16.5))
        # Slices: fenced on the slice list.
        slices = lambda: {"slices": sample_row()["slices"]}
        self.assertEqual(mutate_through(bridge, "sample.slice", fenced(slices(), action="insert", time=66150), "slice-key-0001")["slices"], [0, 22050, 44100, 66150])
        self.assertEqual(mutate_through(bridge, "sample.slice", fenced(slices(), action="move", time=22050, toTime=11025), "slice-key-0002")["slices"], [0, 11025, 44100, 66150])
        self.assertEqual(mutate_through(bridge, "sample.slice", fenced(slices(), action="remove", time=44100), "slice-key-0003")["slices"], [0, 11025, 66150])
        for fields, message in (({"action": "insert", "time": 0}, "already at that time"), ({"action": "remove", "time": 5}, "no slice is at that time"), ({"action": "move", "time": 0}, "takes time and toTime"), ({"action": "clear", "time": 0}, "takes no time")):
            with self.assertRaisesRegex(ValueError, message): bridge.mapper.invoke("sample.slice", fenced(slices(), **fields))
        simpler.sample.ignore_inserts = True
        with self.assertRaisesRegex(ValueError, "^slice insert was not confirmed$"): bridge.mapper.invoke("sample.slice", fenced(slices(), action="insert", time=77))
        self.assertEqual(simpler.sample.slices, [0, 11025, 66150])
        self.assertEqual(mutate_through(bridge, "sample.slice", fenced(slices(), action="reset"), "slice-key-0004")["slices"], [0, 44100])
        for operation in ("device.action", "sample.set", "sample.slice"): self.assertTrue(bridge.mapper._operation_supported(operation), operation)
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("sample.slice"))

    def test_wavetable_settings_and_its_modulation_matrix(self):
        song = family_song(); bridge = immediate_bridge(song); wavetable = song.tracks[0].devices[2]; row = bridge.mapper.snapshot()["tracks"][0]["devices"][2]
        def fenced(state, **fields): return {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": state_revision(state), **fields}
        settings = lambda: {field: bridge.mapper.get(row["ref"])["wavetable"][field] for field in LiveObjectMapper._WAVETABLE_FIELDS}
        # The index is named first, yet the category goes first (a new category starts the index over).
        self.assertTrue(mutate_through(bridge, "wavetable.set", fenced(settings(), oscillator1WavetableIndex=4, oscillator1WavetableCategory=2, unisonVoiceCount=4), "wavetable-key-0001")["changed"])
        self.assertEqual((wavetable.oscillator_1_wavetable_category, wavetable.oscillator_1_wavetable_index, wavetable.unison_voice_count), (2, 4, 4))
        targets = lambda: {"targets": bridge.mapper.get(row["ref"])["wavetable"]["visibleModulationTargetNames"]}
        result = mutate_through(bridge, "wavetable.modulation.set", fenced(targets(), targetIndex=1, source=2, value=0.5), "modulation-key-0001")
        self.assertEqual({key: result[key] for key in ("targetIndex", "value", "prior")}, {"targetIndex": 1, "value": 0.5, "prior": 0.0})
        # A parameter not in the matrix yet is added to it, then modulated.
        parameter_ref = bridge.mapper.get(row["ref"])["parameters"][1]["ref"]
        result = mutate_through(bridge, "wavetable.modulation.set", fenced(targets(), parameterRef=parameter_ref, source=0, value=-0.25), "modulation-key-0002")
        self.assertEqual((result["targetIndex"], wavetable.visible_modulation_target_names[-1], wavetable.amounts[(2, 0)]), (2, "Filter 2 Freq", -0.25))
        other = bridge.mapper.get(bridge.mapper.snapshot()["tracks"][0]["devices"][1]["ref"])["parameters"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "one of this Wavetable's parameters"): bridge.mapper.invoke("wavetable.modulation.set", fenced(targets(), parameterRef=other, source=0, value=0.1))
        with self.assertRaisesRegex(ValueError, "exactly one of targetIndex or parameterRef"): bridge.mapper.invoke("wavetable.modulation.set", fenced(targets(), source=0, value=0.1))
        with self.assertRaisesRegex(ValueError, "not one of the matrix's targets"): bridge.mapper.invoke("wavetable.modulation.set", fenced(targets(), targetIndex=7, source=0, value=0.1))
        self.assertTrue(bridge.mapper._operation_supported("wavetable.set") and bridge.mapper._operation_supported("wavetable.modulation.set"))

    def test_drift_matrix_looper_lengths_and_the_last_rack_variation(self):
        song = FakeSong(); drift = FakeDevice(); drift.name = "Drift"; drift.class_name = "Drift"; drift.pitch_bend_range = 12; drift.voice_count_index = 2; drift.voice_mode_index = 0
        for attribute in LiveObjectMapper._DRIFT_MOD_FIELDS.values(): setattr(drift, attribute, 0)
        drift.mod_matrix_source_1_list = ["Env 1", "LFO", "Velocity"]; drift.mod_matrix_filter_source_1_list = ["Env 2", "LFO"]
        looper = FakeDevice(); looper.name = "Looper"; looper.class_name = "Looper"; looper.calls = []; looper.double_length = lambda: looper.calls.append("double"); looper.half_length = lambda: looper.calls.append("half")
        rack = FakeRackDevice(); rack.recalled = 0; rack.recall_last_used_variation = lambda: setattr(rack, "recalled", rack.recalled + 1)
        song.tracks[0].devices = [drift, looper, rack]; mapper = LiveObjectMapper(song); rows = mapper.snapshot()["tracks"][0]["devices"]
        self.assertEqual((rows[0]["drift"]["modSources"], rows[0]["drift"]["modFilterSourceList"], rows[0]["drift"]["modSource1"]), (["Env 1", "LFO", "Velocity"], ["Env 2", "LFO"], 0))
        state = {field: rows[0]["drift"].get(field) for field in ("pitchBendRange", "voiceCount", "voiceMode", *LiveObjectMapper._DRIFT_MOD_FIELDS)}
        request = {"ref": rows[0]["ref"], "modSource1": 2, "modTarget3": 1, "expectedObjectIdentity": rows[0]["objectIdentity"], "expectedStateRevision": state_revision(state)}
        validate_operation_payload("drift.set", "request", request); self.assertTrue(mapper.invoke("drift.set", request)["changed"])
        self.assertEqual((drift.mod_matrix_source_1_index, drift.mod_matrix_target_3_index), (2, 1))
        for action in ("double-length", "half-length"):
            request = {"ref": rows[1]["ref"], "action": action, "expectedObjectIdentity": rows[1]["objectIdentity"], "expectedStateRevision": state_revision(mapper._looper_state(looper))}
            validate_operation_payload("looper.action", "request", request); mapper.invoke("looper.action", request)
        self.assertEqual(looper.calls, ["double", "half"])
        request = {"ref": rows[2]["ref"], "action": "recall-last-variation", "expectedObjectIdentity": rows[2]["objectIdentity"], "expectedStateRevision": host_rack_state_revision(rows[2])}
        validate_operation_payload("rack.action", "request", request); self.assertTrue(mapper.invoke("rack.action", request)["done"]); self.assertEqual(rack.recalled, 1)


class PreviewBrowser:
    def __init__(self):
        self.samples = types.SimpleNamespace(name="Samples", children=[types.SimpleNamespace(name="Kick 808.wav", children=[], is_loadable=True), types.SimpleNamespace(name="Snare.wav", children=[], is_loadable=True)])
        self.previews = []
    def preview_item(self, item): self.previews.append(item.name)
    def stop_preview(self): self.previews.append("stop")


class ReadsMessagesAndPreviewTests(unittest.TestCase):
    """plugin.parameter-names, device.banks.read and clip.time-convert (reads), application.message
    (authority-free) and browser.preview.start/stop (named previews)."""

    def test_plug_in_names_max_banks_and_clip_times_are_reads(self):
        song = FakeSong(); names = [f"Param {index}" for index in range(300)]
        plugin = FakeDevice(); plugin.name = "Serum"; plugin.class_name = "PluginDevice"; plugin.get_parameter_names = lambda begin=0, end=-1: names[begin:] if end == -1 else names[begin:end]
        banks = [("Main", [0, 1, -1]), ("Extra", [2])]; max_device = FakeDevice(); max_device.name = "LFO"; max_device.class_name = "MaxDevice"
        max_device.get_bank_count = lambda: len(banks); max_device.get_bank_name = lambda index: banks[index][0]; max_device.get_bank_parameters = lambda index: banks[index][1]
        clip = FakeCapturedAudioClip(); clip.is_recording = False; clip.sample_rate = 48000
        clip.beat_to_sample_time = lambda beats: beats * 24000.0; clip.sample_to_beat_time = lambda samples: samples / 24000.0; clip.seconds_to_sample_time = lambda seconds: seconds * 48000.0
        song.tracks[0].devices = [plugin, max_device]; song.tracks[0].clip_slots[0].clip = clip
        bridge = immediate_bridge(song); row = bridge.mapper.snapshot()["tracks"][0]; plugin_ref, max_ref, clip_ref = row["devices"][0]["ref"], row["devices"][1]["ref"], row["clips"][0]["ref"]
        for operation in ("plugin.parameter-names", "device.banks.read", "clip.time-convert"):
            self.assertFalse(remote_module._mutation_authority_required(operation), operation); self.assertTrue(bridge.mapper._operation_supported(operation), operation)
            self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported(operation), operation)
        self.assertEqual(read_through(bridge, "plugin.parameter-names", {"ref": plugin_ref}), {"names": names, "total": 300})
        self.assertEqual(read_through(bridge, "plugin.parameter-names", {"ref": plugin_ref, "begin": 10, "end": 12}), {"names": ["Param 10", "Param 11"], "total": None})
        with self.assertRaisesRegex(ValueError, "only a plug-in"): bridge.mapper.invoke("plugin.parameter-names", {"ref": max_ref})
        with self.assertRaisesRegex(ValueError, "arguments are invalid"): bridge.mapper.invoke("plugin.parameter-names", {"ref": plugin_ref, "begin": 5, "end": 2})
        self.assertEqual(read_through(bridge, "device.banks.read", {"ref": max_ref}), {"banks": [{"name": "Main", "parameters": [0, 1, -1]}, {"name": "Extra", "parameters": [2]}]})
        with self.assertRaisesRegex(ValueError, "only a Max for Live device"): bridge.mapper.invoke("device.banks.read", {"ref": plugin_ref})
        self.assertEqual(read_through(bridge, "clip.time-convert", {"ref": clip_ref, "from": "beats", "value": 2}), {"beats": 2.0, "samples": 48000.0, "seconds": 1.0})
        self.assertEqual(read_through(bridge, "clip.time-convert", {"ref": clip_ref, "from": "seconds", "value": 0.5}), {"beats": 1.0, "samples": 24000.0, "seconds": 0.5})
        self.assertEqual(read_through(bridge, "clip.time-convert", {"ref": clip_ref, "from": "samples", "value": 12000}), {"beats": 0.5, "samples": 12000.0, "seconds": 0.25})
        # Live can't convert (an unwarped sample has no beat time): null, not a guess.
        def unwarped(_value): raise RuntimeError("the sample is not warped")
        clip.beat_to_sample_time = unwarped; clip.sample_to_beat_time = unwarped
        self.assertEqual(read_through(bridge, "clip.time-convert", {"ref": clip_ref, "from": "beats", "value": 2}), {"beats": 2.0, "samples": None, "seconds": None})
        song.tracks[0].clip_slots[0].clip = FakeNoteClip(4.0); midi_ref = bridge.mapper.snapshot()["tracks"][0]["clips"][0]["ref"]
        with self.assertRaisesRegex(ValueError, "only an audio clip"): bridge.mapper.invoke("clip.time-convert", {"ref": midi_ref, "from": "beats", "value": 1})

    def test_a_message_in_live_needs_no_mutation_authority(self):
        shown = []; application = types.SimpleNamespace(show_on_the_fly_message=lambda text: shown.append(("passing", text)), show_message=lambda text: shown.append(("modal", text)))
        bridge = immediate_bridge(FakeSong()); bridge.mapper._application = lambda: application
        self.assertIn("application.message", remote_module._AUTHORITY_FREE_INVOKES); self.assertNotIn("application.message", remote_module._READ_ONLY_INVOKES)
        remote = AuthenticatedRemoteScript("0123456789abcdef0123456789abcdef", lambda method, frame: bridge._dispatch_with_holder(method, frame, {}))
        unsigned = remote.bound({"version": PROTOCOL, "id": "message", "method": "invoke", "operation": "application.message", "args": {"text": "Bounced the drums"}, "nonce": "message-nonce-0001", "sequence": 1})
        answer = remote.dispatch({**unsigned, "mac": remote.sign(unsigned)})
        self.assertTrue(answer["ok"], answer); self.assertEqual(answer["result"], {"shown": True})
        self.assertEqual(bridge._dispatch_with_holder("invoke", {"operation": "application.message", "args": {"text": "Check the mix", "modal": True}}, {}), {"shown": True})
        self.assertEqual(shown, [("passing", "Bounced the drums"), ("modal", "Check the mix")])
        with self.assertRaisesRegex(ValueError, "message arguments are invalid"): bridge.mapper.invoke("application.message", {"text": ""})
        self.assertTrue(bridge.mapper._operation_supported("application.message"))
        # The undo steps keep their own probe.
        self.assertFalse(bridge.mapper._operation_supported("undo.step.begin"))
        def unavailable(): raise ValueError("Live's application is unavailable")
        bridge.mapper._application = unavailable; self.assertFalse(bridge.mapper._operation_supported("application.message"))

    def test_a_preview_is_named_and_only_its_own_name_stops_it(self):
        browser = PreviewBrowser(); bridge = immediate_bridge(FakeSong()); bridge.mapper._browser = lambda: browser
        self.assertTrue(bridge.mapper._operation_supported("browser.preview.start") and bridge.mapper._operation_supported("browser.preview.stop"))
        kick, snare = (bridge.mapper.invoke("browser.inspect", {"itemId": f"samples/{name}"}) for name in ("Kick 808.wav", "Snare.wav"))
        start = lambda item, key: mutate_through(bridge, "browser.preview.start", {"itemId": item["id"], "expectedName": item["name"], "expectedItemIdentity": item["objectIdentity"]}, key)
        first = start(kick, "preview-key-0001"); self.assertTrue(first["started"]); self.assertGreaterEqual(len(first["previewId"]), 32)
        second = start(snare, "preview-key-0002")
        with self.assertRaisesRegex(ValueError, "isn't playing any more"): bridge.mapper.invoke("browser.preview.stop", {"previewId": first["previewId"]})
        self.assertEqual(mutate_through(bridge, "browser.preview.stop", {"previewId": second["previewId"]}, "preview-key-0003"), {"stopped": True})
        self.assertEqual(browser.previews, ["Kick 808.wav", "Snare.wav", "stop"])
        with self.assertRaisesRegex(ValueError, "identity changed since it was found"): bridge.mapper.invoke("browser.preview.start", {"itemId": kick["id"], "expectedName": "Kick 909.wav", "expectedItemIdentity": kick["objectIdentity"]})
        self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("browser.preview.start"))


def duplicable(owner, append=False):
    """Give a track or chain Live's duplicate_device (the copy right after the device) and delete_device."""
    def duplicate_device(index):
        source = owner.devices[index]; copy = FakeDevice(); copy.name = source.name; copy.class_name = source.class_name
        owner.devices.insert(len(owner.devices) if append else index + 1, copy)
    owner.duplicate_device = duplicate_device; owner.delete_device = lambda index: owner.devices.pop(index)
    return owner


class DeviceDuplicationTests(unittest.TestCase):
    """device.duplicate: a transaction creation fenced on the device, its owner and siblings."""

    def request(self, mapper, device_row, owner_row, siblings, **fields):
        return {"ref": device_row["ref"], "expectedName": device_row["name"], "expectedObjectIdentity": device_row["objectIdentity"], "expectedOwnerRef": owner_row["ref"], "expectedOwnerIdentity": owner_row["objectIdentity"], "expectedSiblings": [{"ref": row["ref"], "objectIdentity": row["objectIdentity"]} for row in siblings], **fields}

    def test_a_copy_lands_right_after_the_device_and_its_transaction_can_take_it_away(self):
        song = FakeSong(); track = duplicable(song.tracks[0]); eq, comp = FakeDevice(), FakeDevice(); eq.name = "EQ Eight"; comp.name = "Compressor"; track.devices = [eq, comp]
        bridge = immediate_bridge(song, provenance="real-live"); row = bridge.mapper.snapshot()["tracks"][0]
        self.assertTrue(bridge.mapper._operation_supported("device.duplicate")); self.assertFalse(LiveObjectMapper(FakeSong())._operation_supported("device.duplicate"))
        made = mutate_through(bridge, "device.duplicate", self.request(bridge.mapper, row["devices"][0], row, row["devices"]), "duplicate-key-0001", transaction="transaction-duplicate")
        self.assertEqual((made["ref"], made["name"], made["index"]), (f"{bridge.mapper.refs.epoch}:device:0:1", "EQ Eight", 1))
        self.assertEqual([device.name for device in track.devices], ["EQ Eight", "EQ Eight", "Compressor"]); self.assertIs(track.devices[0], eq)
        self.assertRegex(made["ownershipToken"], r"^[A-Za-z0-9_-]{32,128}$"); self.assertEqual(made["createdFingerprint"], bridge.mapper._ownership_fingerprint(made["ref"]))
        # Undo: the transaction that made the copy deletes it with its ownership.
        row = bridge.mapper.snapshot()["tracks"][0]; copy_row = row["devices"][1]
        delete = {"ref": copy_row["ref"], "expectedObjectIdentity": copy_row["objectIdentity"], "expectedOwnerRef": row["ref"], "expectedOwnerIdentity": row["objectIdentity"], "expectedSiblings": [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in row["devices"]], "expectedTrackRef": row["ref"], "expectedTrackIdentity": row["objectIdentity"]}
        answer = bridge._dispatch_with_holder("mutate", {"operation": "device.delete", "transactionId": "transaction-duplicate", "idempotencyKey": "undo-duplicate-0001", "ownershipToken": made["ownershipToken"], "args": delete}, {})
        self.assertEqual(answer, {"deleted": copy_row["ref"]}); self.assertEqual(track.devices, [eq, comp])

    def test_a_misplaced_copy_is_taken_away_and_the_fences_hold(self):
        song = FakeSong(); track = duplicable(song.tracks[0], append=True); eq, comp = FakeDevice(), FakeDevice(); eq.name = "EQ Eight"; comp.name = "Compressor"; track.devices = [eq, comp]
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        with self.assertRaisesRegex(ValueError, "isn't right after the device"): mapper.invoke("device.duplicate", self.request(mapper, row["devices"][0], row, row["devices"]))
        self.assertEqual(track.devices, [eq, comp])
        with self.assertRaisesRegex(ValueError, "takes its identity, owner and siblings"): mapper.invoke("device.duplicate", {"ref": row["devices"][0]["ref"]})
        with self.assertRaisesRegex(ValueError, "device name changed since preview"): mapper.invoke("device.duplicate", self.request(mapper, row["devices"][0], row, row["devices"], expectedName="Old Name"))
        with self.assertRaisesRegex(ValueError, "owner or siblings changed"): mapper.invoke("device.duplicate", self.request(mapper, row["devices"][0], row, row["devices"][:1]))

    def test_an_instrument_isnt_copied_and_a_copy_live_refuses_leaves_the_chain_as_it_was(self):
        # Measured on real Live: duplicate_device on Drift raises RuntimeError (a chain holds one instrument).
        song = FakeSong(); track = duplicable(song.tracks[0]); drift, saturator = FakeDevice(), FakeDevice(); drift.name = "Drift"; drift.type = 1; saturator.name = "Saturator"; saturator.type = 2
        track.devices = [drift, saturator]; asked = []
        original = track.duplicate_device
        track.duplicate_device = lambda index: (asked.append(index), original(index))
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        with self.assertRaisesRegex(ValueError, r'^a chain holds one instrument, so Live can\'t copy "Drift" beside itself: duplicate its track instead; nothing changed$'):
            mapper.invoke("device.duplicate", self.request(mapper, row["devices"][0], row, row["devices"]))
        self.assertEqual((asked, track.devices), ([], [drift, saturator]))
        def refuses(index): raise RuntimeError("Live's own words")
        track.duplicate_device = refuses
        with self.assertRaisesRegex(ValueError, r'^Live refused to copy "Saturator"; nothing changed$'):
            mapper.invoke("device.duplicate", self.request(mapper, row["devices"][1], row, row["devices"]))
        self.assertEqual(track.devices, [drift, saturator])
        self.assertEqual(remote_module._failure_summary(ValueError('Live refused to copy "Saturator"; nothing changed')), 'request failed: Live refused to copy "Saturator"; nothing changed')

    def test_a_device_in_a_rack_chain_is_copied_within_its_chain(self):
        song = FakeSong(); rack = FakeRackDevice(); inner = FakeDevice(); inner.name = "Saturator"
        chain = duplicable(type("Chain", (), {"name": "Chain 1", "mute": False, "solo": False})()); chain.devices = [inner]; rack.chains = [chain]; song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song); rack_row = mapper.snapshot()["tracks"][0]["devices"][0]; chain_row = rack_row["chains"][0]; device_row = chain_row["devices"][0]
        made = mapper.invoke("device.duplicate", self.request(mapper, device_row, chain_row, chain_row["devices"]))
        validate_operation_payload("device.duplicate", "result", made)
        self.assertEqual((made["ref"], [device.name for device in chain.devices]), (f"{mapper.refs.epoch}:device:0:0:0:1", ["Saturator", "Saturator"]))
        self.assertIs(mapper._device_owner_of(made["ref"]), chain)


class ExtendedOperationTests(unittest.TestCase):
    """song.set selectOnLaunch, song.read's and tuning.read's new fields, track.view.set showChains,
    clip.view.set envelopeParameterRef and device.parameter.set gesture."""

    @staticmethod
    def host_song_settings(read):
        """The song settings fence as the host computes it from song.read."""
        settings = {field: read.get(field) for field in ("signatureNumerator", "signatureDenominator", "swingAmount", "selectOnLaunch")}
        settings.update({field: (read.get(field) or {}).get("value") for field in ("clipTriggerQuantization", "midiRecordingQuantization")})
        return state_revision(settings)

    def test_select_on_launch_and_what_song_read_adds(self):
        song = FakeSong(); song.clip_trigger_quantization = type("Quantization", (int,), {"name": "q_bar"})(4); song.signature_numerator = 4; song.signature_denominator = 4; song.select_on_launch = False; song.last_event_time = 64.0; song.session_record_status = 0
        song.can_jump_to_next_cue = True; song.can_jump_to_prev_cue = False; song.is_cue_point_selected = lambda: False
        song.get_current_beats_song_time = lambda: types.SimpleNamespace(bars=3, beats=2, sub_division=1, ticks=0)
        bridge = immediate_bridge(song); snapshot = bridge.mapper.snapshot(); set_ref = snapshot["set"]["ref"]
        read = read_through(bridge, "song.read", {"setRef": set_ref})
        self.assertEqual({key: read[key] for key in ("lastEventTime", "sessionRecordStatus", "canJumpToNextCue", "canJumpToPrevCue", "isCuePointSelected", "selectOnLaunch", "beatsSongTime")}, {"lastEventTime": 64.0, "sessionRecordStatus": 0, "canJumpToNextCue": True, "canJumpToPrevCue": False, "isCuePointSelected": False, "selectOnLaunch": False, "beatsSongTime": "3.2.1.0"})
        # The playhead moving doesn't move the revision; a setting does.
        song.get_current_beats_song_time = lambda: types.SimpleNamespace(bars=5, beats=1, sub_division=1, ticks=0)
        self.assertEqual(bridge.mapper.invoke("song.read", {"setRef": set_ref})["revision"], read["revision"])
        request = {"setRef": set_ref, "expectedObjectIdentity": snapshot["set"]["objectIdentity"], "selectOnLaunch": True, "expectedStateRevision": self.host_song_settings(read)}
        self.assertTrue(mutate_through(bridge, "song.set", request, "song-set-0001")["changed"]); self.assertIs(song.select_on_launch, True)
        self.assertNotEqual(bridge.mapper.invoke("song.read", {"setRef": set_ref})["revision"], read["revision"])
        with self.assertRaisesRegex(ValueError, "song settings state changed since preview"): bridge.mapper.invoke("song.set", {**request, "selectOnLaunch": False})
        with self.assertRaisesRegex(ValueError, "selectOnLaunch is invalid"): bridge.mapper.invoke("song.set", {**request, "selectOnLaunch": 1, "expectedStateRevision": self.host_song_settings(bridge.mapper.invoke("song.read", {"setRef": set_ref}))})

    def test_tuning_read_gives_the_reference_pitch_and_the_pseudo_octave(self):
        song = FakeSong(); song.tuning_system = FakeTuningSystem(); song.root_note = 0; song.scale_name = "Major"; song.scale_mode = True; song.scale_intervals = [0, 2, 4, 5, 7, 9, 11]
        song.tuning_system.reference_pitch = types.SimpleNamespace(frequency=432.0, index_in_octave=9, octave=4); song.tuning_system.number_of_notes_in_pseudo_octave = 12
        mapper = LiveObjectMapper(song); set_ref = mapper.snapshot()["set"]["ref"]
        read = mapper.invoke("tuning.read", {"setRef": set_ref}); validate_operation_payload("tuning.read", "result", read)
        self.assertEqual((read["referencePitch"], read["notesInPseudoOctave"]), ({"frequency": 432.0, "indexInOctave": 9, "octave": 4}, 12))
        # The revision covers them: a new reference pitch is a new revision.
        song.tuning_system.reference_pitch = types.SimpleNamespace(frequency=440.0, index_in_octave=9, octave=4)
        self.assertNotEqual(mapper.invoke("tuning.read", {"setRef": set_ref})["revision"], read["revision"])
        song.tuning_system.reference_pitch = {"note": 69, "frequency": 440.0}
        self.assertIsNone(mapper.invoke("tuning.read", {"setRef": set_ref})["referencePitch"])

    def test_a_track_shows_its_racks_chains_and_a_clip_shows_a_parameters_envelope(self):
        song = FakeSong(); track = song.tracks[0]; track.view = types.SimpleNamespace(is_collapsed=False, device_insert_mode=0); track.is_showing_chains = False; track.can_show_chains = True
        shown = []; clip = FakeClip(4.0); clip.view = types.SimpleNamespace(grid_quantization=4, grid_is_triplet=False, select_envelope_parameter=lambda parameter: shown.append(parameter)); track.clip_slots[0].clip = clip
        other = FakeTrack(); song.tracks.append(other)
        bridge = immediate_bridge(song); snapshot = bridge.mapper.snapshot(); row = snapshot["tracks"][0]
        view_state = lambda chains: state_revision({"collapsed": False, "deviceInsertMode": 0, "showChains": chains})
        request = {"ref": row["ref"], "showChains": True, "expectedObjectIdentity": row["objectIdentity"], "expectedStateRevision": view_state(False)}
        self.assertTrue(mutate_through(bridge, "track.view.set", request, "track-view-0001")["changed"]); self.assertIs(track.is_showing_chains, True)
        with self.assertRaisesRegex(ValueError, "track view state changed since preview"): bridge.mapper.invoke("track.view.set", request)
        track.can_show_chains = False
        with self.assertRaisesRegex(ValueError, "no Instrument Rack"): bridge.mapper.invoke("track.view.set", {**request, "showChains": False, "expectedStateRevision": view_state(True)})
        clip_row = row["clips"][0]; parameter = row["devices"][0]["parameters"][0]; foreign = snapshot["tracks"][1]["devices"][0]["parameters"][0]
        clip_request = {"ref": clip_row["ref"], "envelopeParameterRef": parameter["ref"], "expectedObjectIdentity": clip_row["objectIdentity"], "expectedStateRevision": state_revision({"gridQuantization": 4, "gridIsTriplet": False})}
        self.assertTrue(mutate_through(bridge, "clip.view.set", clip_request, "clip-view-0001")["changed"]); self.assertEqual(shown, [track.devices[0].parameters[0]])
        with self.assertRaisesRegex(ValueError, "parameter on the clip's own track"): bridge.mapper.invoke("clip.view.set", {**clip_request, "envelopeParameterRef": foreign["ref"]})
        self.assertEqual(len(shown), 1)

    def test_a_gesture_begins_and_always_ends(self):
        calls = []; bridge = immediate_bridge(); parameter_object = bridge.mapper.song.tracks[0].devices[0].parameters[0]
        parameter_object.begin_gesture = lambda: calls.append("begin"); parameter_object.end_gesture = lambda: calls.append("end")
        parameter = bridge.mapper.snapshot()["tracks"][0]["devices"][0]["parameters"][0]
        args = lambda value: {"ref": parameter["ref"], "value": value, "gesture": True, "expectedRevision": bridge.mapper.refs.revision(parameter["ref"]), **ControlSurfaceTests.parameter_authority(bridge.mapper, parameter["ref"])}
        self.assertEqual(mutate_through(bridge, "device.parameter.set", args(0.75), "gesture-key-0001")["value"], 0.75); self.assertEqual(calls, ["begin", "end"])
        # Past its range: held at the top of it.
        self.assertEqual(bridge.mapper.invoke("device.parameter.set", args(2.0))["value"], 1.0)
        self.assertEqual(calls, ["begin", "end", "begin", "end"])
        def stuck(): raise RuntimeError("Live kept the gesture")
        parameter_object.end_gesture = stuck
        with self.assertRaisesRegex(ValueError, "gesture didn't end"): bridge.mapper.invoke("device.parameter.set", args(0.25))
        del parameter_object.begin_gesture
        with self.assertRaisesRegex(ValueError, "gestures are unavailable"): bridge.mapper.invoke("device.parameter.set", args(0.5))


class AuditRowFieldTests(unittest.TestCase):
    """The cheap reads the LOM audit found, as fields of existing rows."""

    def test_devices_racks_clips_and_tracks_carry_what_live_says_about_them(self):
        song = FakeSong(); track = song.tracks[0]; track.can_be_frozen = True; track.is_grouped = False; track.is_showing_chains = False; track.is_part_of_selection = True
        track.devices[0].class_display_name = "Utility"
        rack = FakeRackDevice(); rack.has_macro_mappings = True; rack.macros_mapped = (True, False); rack.is_showing_chains = True; track.devices.append(rack)
        audio = FakeCapturedAudioClip(); audio.is_recording = False; audio.gain_display_string = "-3.0 dB"; audio.sample_rate = 44100; audio.is_overdubbing = False; audio.has_envelopes = True
        track.clip_slots = [FakeSlot(), FakeSlot()]; track.clip_slots[0].clip = audio; midi = FakeNoteClip(4.0); midi.gain_display_string = "0.0 dB"; midi.has_envelopes = False; track.clip_slots[1].clip = midi
        mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][0]
        self.assertEqual((row["canBeFrozen"], row["isGrouped"], row["view"]["isShowingChains"], row["view"]["isPartOfSelection"]), (True, False, False, True))
        self.assertEqual((row["devices"][0]["classDisplayName"], row["devices"][1]["hasMacroMappings"], row["devices"][1]["macrosMapped"], row["devices"][1]["view"]["isShowingChains"]), ("Utility", True, [True, False], True))
        audio_row, midi_row = row["clips"]
        self.assertEqual({key: audio_row[key] for key in ("gainDisplay", "sampleRate", "isOverdubbing", "hasEnvelopes")}, {"gainDisplay": "-3.0 dB", "sampleRate": 44100.0, "isOverdubbing": False, "hasEnvelopes": True})
        self.assertEqual((midi_row["gainDisplay"], midi_row["sampleRate"], midi_row["hasEnvelopes"]), (None, None, False))
        # Rows stay within what get and discover may return.
        self.assertLessEqual(len(row), 64); self.assertLessEqual(max(len(audio_row), len(midi_row)), 64)
        for reference in (audio_row["ref"], row["devices"][1]["ref"], row["ref"]): validate_operation_payload("get", "result", mapper.get(reference))

    def test_selection_and_shown_chains_leave_a_created_tracks_fingerprint_alone(self):
        song = FakeSong(); mapper = LiveObjectMapper(song, provenance="real-live")
        created = mapper.invoke("track.create", {"kind": "midi", "index": 1, "name": "Made", "expectedStructureRevision": mapper._structure_revision()}, "transaction-rows")
        made = song.tracks[1]; made.is_part_of_selection = False; made.is_showing_chains = False; before = mapper._ownership_fingerprint(created["ref"])
        made.is_part_of_selection = True; made.is_showing_chains = True
        self.assertEqual(mapper._ownership_fingerprint(created["ref"]), before)

    def test_the_set_says_where_modulation_mapping_is(self):
        song = FakeSong(); device = song.tracks[0].devices[0]; parameter = device.parameters[0]
        song.view = types.SimpleNamespace(selected_track=song.tracks[0], mod_mapping_device=device, mod_mapping_parameter=parameter)
        mapper = LiveObjectMapper(song); snapshot = mapper.snapshot(); set_row = snapshot["set"]
        self.assertEqual((set_row["modMappingDeviceRef"], set_row["modMappingParameterRef"]), (snapshot["tracks"][0]["devices"][0]["ref"], snapshot["tracks"][0]["devices"][0]["parameters"][0]["ref"]))
        song.view.mod_mapping_device = None; song.view.mod_mapping_parameter = None
        self.assertEqual({key: mapper.snapshot()["set"][key] for key in ("modMappingDeviceRef", "modMappingParameterRef")}, {"modMappingDeviceRef": None, "modMappingParameterRef": None})
        # A Live without those says nothing about them.
        self.assertNotIn("modMappingDeviceRef", LiveObjectMapper(FakeSong()).snapshot()["set"])

    def test_status_names_lives_version_and_variant(self):
        application = types.SimpleNamespace(get_version_string=lambda: "12.4.15b5", get_variant=lambda: "Suite", unavailable_features=["push_apps"])
        live = types.SimpleNamespace(Application=types.SimpleNamespace(get_application=lambda: application))
        with patch.dict(sys.modules, {"Live": live}):
            environment = LiveObjectMapper(FakeSong())._environment_probe()
        self.assertEqual((environment["liveVersion"], environment["liveEdition"], environment["unavailableFeatures"]), ("12.4.15b5", "Suite", ["push_apps"]))


class LeanParameter:
    __slots__ = ("name", "value", "min", "max", "is_quantized", "is_enabled", "automation_state", "default_value")

    def __init__(self, name):
        self.name = name; self.value = 0.5; self.min = 0.0; self.max = 1.0; self.is_quantized = False; self.is_enabled = True; self.automation_state = 0; self.default_value = 0.5


class LeanChain:
    def __init__(self, name, devices):
        self.name = name; self.devices = devices; self.mute = False; self.solo = False


class LeanDevice:
    """A device as a big Set has thousands: parameters, and chains when it's a rack."""
    parameter_reads = 0

    def __init__(self, name, class_name, parameters, chains=None, kind=2):
        self.name = name; self.class_name = class_name; self._parameters = [LeanParameter(f"{name} {index + 1}") for index in range(parameters)]
        self.is_active = True; self.type = kind; self.can_have_chains = chains is not None; self.can_have_drum_pads = False
        if chains is not None:
            self.chains = chains; self.return_chains = []; self.macros = self._parameters[1:9]; self.visible_macro_count = 8; self.variation_count = 0; self.selected_variation_index = -1

    @property
    def parameters(self):
        LeanDevice.parameter_reads += 1
        return self._parameters


class LeanPad:
    def __init__(self, name, chains):
        self.name = name; self.chains = chains; self.mute = False; self.solo = False; self.note = 36


def lean_template_devices():
    """The measured Set's template: Operator, EQ Eight, Compressor, Reverb, and an Audio Effect Rack
    with two chains (Saturator + Auto Filter; a nested rack with Utility). Nine devices."""
    nested = LeanDevice("Audio Effect Rack", "AudioEffectGroupDevice", 17, [LeanChain("Chain", [LeanDevice("Utility", "StereoGain", 20)])])
    rack = LeanDevice("Audio Effect Rack", "AudioEffectGroupDevice", 17, [LeanChain("Drive", [LeanDevice("Saturator", "Saturator", 25), LeanDevice("Auto Filter", "AutoFilter", 40)]), LeanChain("Nest", [nested])])
    return [LeanDevice("Operator", "Operator", 195, kind=1), LeanDevice("EQ Eight", "Eq8", 87), LeanDevice("Compressor", "Compressor2", 30), LeanDevice("Reverb", "Reverb", 37), rack]


def lean_track(name, devices=None):
    track = FakeTrack(); track.name = name; track.clip_slots = [FakeSlot() for _ in range(8)]; track.mixer_device = FakeMixerDevice(); track.arrangement_clips = []
    track.devices = lean_template_devices() if devices is None else devices
    return track


def drum_rack():
    """A Drum Rack whose pads' chains the rack lists, and one pad with a chain it doesn't."""
    kick, snare = LeanChain("Kick", [LeanDevice("Simpler", "OriginalSimpler", 10)]), LeanChain("Snare", [LeanDevice("Simpler", "OriginalSimpler", 10)])
    rack = LeanDevice("Drum Rack", "DrumGroupDevice", 17, [kick, snare]); rack.can_have_drum_pads = True
    rack.visible_drum_pads = [LeanPad("Kick", [kick]), LeanPad("Snare", [snare]), LeanPad("Hat", [LeanChain("Hat", [LeanDevice("Simpler", "OriginalSimpler", 10)])])]
    return rack


class LightDeviceDiscoveryTests(unittest.TestCase):
    """A: devices are listed from light rows, which read no parameters; whole rows only for a page
    whose fields need them. What's listed is exactly what whole-row discovery listed."""

    FIELDS = ["parentRef", "name", "className", "chainList"]

    @staticmethod
    def whole_items(mapper, parent=None):
        """Device discovery's items as they were built from whole track rows."""
        rows = [{**device, "chainList": [{"ref": chain["ref"], "name": chain.get("name")} for chain in device["chains"]]} if device.get("chains") else device for track in mapper.snapshot()["tracks"] for device in mapper._flatten_device_rows(track["devices"])]
        return [row for row in rows if parent is None or row.get("parentRef") == parent]

    @staticmethod
    def only(rows, fields):
        allowed = set(fields) | {"ref", "parentRef"}
        return [{key: value for key, value in row.items() if key in allowed} for row in rows]

    def set_with_racks(self):
        song = FakeSong(); song.tracks = [lean_track("Synth"), lean_track("Drums", [drum_rack(), LeanDevice("Glue", "GlueCompressor", 12)]), lean_track("Empty", [])]
        return song, LiveObjectMapper(song)

    def test_a_device_list_reads_no_parameters_and_lists_what_whole_rows_listed(self):
        song, mapper = self.set_with_racks(); expected = self.only(self.whole_items(mapper), self.FIELDS)
        LeanDevice.parameter_reads = 0
        listed = mapper.discover("device", 1000, None, None, None, self.FIELDS)
        self.assertEqual(LeanDevice.parameter_reads, 0); self.assertEqual(listed["items"], expected)
        self.assertEqual(len(expected), 9 + 5)  # a template track, and a Drum Rack's three Simplers (the unlisted pad's too) with a Glue Compressor
        validate_operation_payload("discover", "result", listed)
        # A track's devices and a chain's, as whole rows listed them.
        row = mapper.snapshot()["tracks"][0]; rack = row["devices"][4]
        for parent in (row["ref"], rack["chains"][0]["ref"], rack["chains"][1]["ref"], rack["chains"][1]["devices"][0]["chains"][0]["ref"]):
            self.assertEqual(mapper.discover("device", 1000, None, parent, None, self.FIELDS)["items"], self.only(self.whole_items(mapper, parent), self.FIELDS), parent)
        # A filter on a light field stays light; on what only a whole row has, whole rows filter.
        LeanDevice.parameter_reads = 0
        self.assertEqual([item["name"] for item in mapper.discover("device", 1000, None, None, {"className": "OriginalSimpler"}, self.FIELDS)["items"]], ["Simpler"] * 3)
        self.assertEqual(LeanDevice.parameter_reads, 0)
        self.assertEqual(mapper.discover("device", 1000, None, None, {"latencySamples": None}, self.FIELDS)["items"], self.only([row for row in self.whole_items(mapper) if row.get("latencySamples") is None], self.FIELDS))

    def test_whole_rows_come_for_the_page_alone_and_the_revision_follows_identities(self):
        song, mapper = self.set_with_racks(); whole = self.whole_items(mapper)
        LeanDevice.parameter_reads = 0
        first = mapper.discover("device", 2)
        self.assertEqual(first["items"], whole[:2]); self.assertEqual(LeanDevice.parameter_reads, 2)
        pages, cursor = [first["items"]], first.get("nextCursor")
        while cursor:
            page = mapper.discover("device", 2, cursor); pages.append(page["items"]); cursor = page.get("nextCursor")
        self.assertEqual([item for page in pages for item in page], whole)
        # A parameter moving isn't a new list; a renamed device is.
        revision = first["revision"]; song.tracks[0].devices[0]._parameters[0].value = 0.9
        self.assertEqual(mapper.discover("device", 2)["revision"], revision)
        song.tracks[0].devices[0].name = "FM"
        self.assertNotEqual(mapper.discover("device", 2)["revision"], revision)
        with self.assertRaisesRegex(ValueError, "invalid discovery cursor"): mapper.discover("device", 2, first["nextCursor"])


class CountingNoteClip(FakeNoteClip):
    """A note clip that counts how often its notes are read."""

    def __init__(self, length=4.0, notes=()):
        super().__init__(length, notes); self.note_reads = 0

    def get_all_notes_extended(self):
        self.note_reads += 1
        return super().get_all_notes_extended()


class TargetedDiscoveryTests(unittest.TestCase):
    """B: slots and clips read their track's slots alone, parameters their device alone, notes their
    clip alone (Arrangement clips too); Arrangement clip rows hold their notes only when asked."""

    def song(self):
        song = FakeSong(); song.tracks = [lean_track("Keys"), lean_track("Bass")]
        session = CountingNoteClip(4.0, [FakeMidiNote(index + 1, 60 + index, index * 0.5, 0.25) for index in range(6)]); song.tracks[0].clip_slots[1].clip = session
        long_clip = CountingNoteClip(64.0, [FakeMidiNote(index + 1, 36 + index % 24, index * 0.25, 0.25) for index in range(50)]); long_clip.start_time = 8.0
        song.tracks[1].arrangement_clips = [long_clip]
        return song, session, long_clip

    def test_slots_clips_and_parameters_read_only_their_parent(self):
        song, session, _ = self.song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot()["tracks"]
        track_ref, slot_ref = whole[0]["ref"], whole[0]["clipSlots"][1]["ref"]
        counter = ReadCounter(song.tracks); LeanDevice.parameter_reads = 0; session.note_reads = 0
        self.assertEqual(mapper.discover("clip_slot", 100, None, track_ref)["items"], whole[0]["clipSlots"])
        self.assertEqual(counter.reads, {0: {"clip_slots"}}); self.assertEqual(LeanDevice.parameter_reads, 0)
        # A clip's notes are read when its fields want them, not otherwise.
        light = mapper.discover("session_clip", 1, None, slot_ref, None, ["name", "length", "isAudio"])["items"]
        self.assertEqual((light, session.note_reads), ([{"ref": whole[0]["clips"][0]["ref"], "parentRef": slot_ref, "name": "", "length": 4.0, "isAudio": False}], 0))
        self.assertEqual(mapper.discover("session_clip", 1, None, slot_ref)["items"], whole[0]["clips"]); self.assertEqual(session.note_reads, 1)
        self.assertEqual(mapper.discover("clip", 5, None, track_ref)["items"], [])  # a clip's parent is its slot
        # A device's parameters, reading that device's alone.
        operator = whole[0]["devices"][0]; LeanDevice.parameter_reads = 0
        self.assertEqual(mapper.discover("parameter", 1000, None, operator["ref"])["items"], operator["parameters"])
        self.assertEqual(LeanDevice.parameter_reads, 1)
        nested = whole[0]["devices"][4]["chains"][1]["devices"][0]["chains"][0]["devices"][0]
        self.assertEqual(mapper.discover("parameter", 1000, None, nested["ref"])["items"], nested["parameters"])

    def test_notes_list_from_their_clip_session_or_arrangement(self):
        song, session, long_clip = self.song(); mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        clip_row = whole["tracks"][0]["clips"][0]
        self.assertEqual(mapper.discover("note", 100, None, clip_row["ref"])["items"], [note | {"ref": f"{clip_row['ref']}:note:{index}", "parentRef": clip_row["ref"]} for index, note in enumerate(clip_row["notes"])])
        arrangement_ref = f"{mapper.refs.epoch}:arrangement_clip:1:0"; notes = mapper._read_notes(long_clip)
        listed, cursor = [], None
        while True:
            page = mapper.discover("note", 20, cursor, arrangement_ref); listed += page["items"]; cursor = page.get("nextCursor")
            validate_operation_payload("discover", "result", page)
            if not cursor: break
        self.assertEqual(listed, [note | {"ref": f"{arrangement_ref}:note:{index}", "parentRef": arrangement_ref} for index, note in enumerate(notes)])
        self.assertEqual(len(listed), 50)
        song.arrangement_clips = [long_clip]; song_level = f"{mapper.refs.epoch}:arrangement_clip:0"
        self.assertEqual(len(mapper.discover("note", 100, None, song_level)["items"]), 50)
        self.assertEqual(mapper.discover("note", 100, None, f"{mapper.refs.epoch}:arrangement_clip:1:7")["items"], [])

    def test_a_clips_note_pages_end_when_the_notes_listed_before_them_moved(self):
        """A note page's cursor holds the notes listed so far: a note gone or added among them ends the
        list (the next page would skip or repeat one); a change past them doesn't."""
        song, _, long_clip = self.song(); mapper = LiveObjectMapper(song)
        clip_ref = mapper.snapshot({"focus": [1], "parts": ["tracks", "arrangement"]})["arrangement"]["clips"][0]["ref"]
        first = mapper.discover("note", 20, None, clip_ref, budgeted=True)
        self.assertEqual([note["id"] for note in first["items"]], list(range(1, 21)))
        long_clip.stored[50].pitch = 99
        second = mapper.discover("note", 20, first["nextCursor"], clip_ref, budgeted=True)
        self.assertEqual([note["id"] for note in second["items"]], list(range(21, 41)))
        self.assertEqual(second["revision"], first["revision"])
        # The first note gone and one added at the end: as many notes, each moved up a place.
        del long_clip.stored[1]; long_clip.stored[51] = FakeMidiNote(51, 40, 15.0, 0.25)
        with self.assertRaisesRegex(ValueError, "invalid discovery cursor"): mapper.discover("note", 20, first["nextCursor"], clip_ref, budgeted=True)

    def test_a_note_page_spends_its_budget_on_notes_not_on_building_the_clips_note_vector(self):
        song, _, long_clip = self.song(); mapper = LiveObjectMapper(song); mapper.read_budget_seconds = 0.01
        clip_ref = mapper.snapshot({"focus": [1], "parts": ["tracks", "arrangement"]})["arrangement"]["clips"][0]["ref"]
        build = long_clip.get_all_notes_extended
        def slow_build():
            time.sleep(0.02); return build()
        long_clip.get_all_notes_extended = slow_build
        self.assertEqual(len(mapper.discover("note", 100, None, clip_ref, budgeted=True)["items"]), 50)

    def test_a_tracks_arrangement_clips_page_on_while_the_set_plays(self):
        """A list of clips binds its cursors to what it lists, not to where playback is: page 2
        follows page 1 while the clips play; a clip that changed ends them."""
        song = FakeSong(); song.tracks = [lean_track("Keys")]
        clips = [FakeClip(4.0) for _ in range(3)]
        for index, clip in enumerate(clips): clip.start_time = index * 4.0; clip.name = f"C{index}"; clip.is_playing = False; clip.playing_position = 0.0
        song.tracks[0].arrangement_clips = clips
        mapper = LiveObjectMapper(song); track_ref = mapper.snapshot()["tracks"][0]["ref"]
        first = mapper.discover("arrangement_clip", 2, None, track_ref)
        clips[0].is_playing = True; clips[0].playing_position = 1.5
        second = mapper.discover("arrangement_clip", 2, first["nextCursor"], track_ref)
        self.assertEqual([item["name"] for item in first["items"] + second["items"]], ["C0", "C1", "C2"])
        self.assertEqual(first["items"][0]["isPlaying"], False)
        clips[1].name = "Renamed"
        with self.assertRaisesRegex(ValueError, "invalid discovery cursor"): mapper.discover("arrangement_clip", 2, first["nextCursor"], track_ref)

    def test_arrangement_clip_rows_hold_their_notes_only_when_asked(self):
        song, _, long_clip = self.song(); mapper = LiveObjectMapper(song); long_clip.note_reads = 0
        row = mapper.snapshot({"focus": [1], "parts": ["tracks", "arrangement"]})["arrangement"]["clips"][0]
        self.assertNotIn("notes", row); self.assertNotIn("notesRevision", row); self.assertEqual(row["noteCount"], 50)
        self.assertEqual(mapper.get(row["ref"])["noteCount"], 50); self.assertNotIn("notes", mapper.get(row["ref"]))
        listed = mapper.discover("arrangement_clip", 10, None, row["parentRef"], None, ["name", "noteCount"])["items"]
        self.assertEqual(listed, [{"ref": row["ref"], "parentRef": row["parentRef"], "name": row["name"], "noteCount": 50}])
        asked = mapper.discover("arrangement_clip", 10, None, row["parentRef"], None, ["notes", "notesRevision"])["items"][0]
        self.assertEqual(len(asked["notes"]), 50); self.assertEqual(asked["notesRevision"], hashlib.sha256(mapper._bounded_canonical(mapper._read_notes(long_clip)).encode()).hexdigest())


def read_all(mapper, kind, parent=None, limit=100000, fields=None, filters=None):
    """Every page of a budgeted discovery, as the host reads them: (items, pages)."""
    items, pages, cursor = [], 0, None
    while True:
        page = mapper.discover(kind, limit, cursor, parent, filters, fields, budgeted=True); pages += 1
        validate_operation_payload("discover", "result", page)
        items += page["items"]; cursor = page.get("nextCursor")
        assert page["truncated"] == (cursor is not None)
        if not cursor: return items, pages


class ReadBudgetTests(unittest.TestCase):
    """C: a read the host asks for holds Live's thread no longer than its budget: it stops after at
    least one unit, saying how to go on."""

    def set(self, tracks=3):
        song = FakeSong(); song.tracks = [lean_track(f"Track {index + 1}") for index in range(tracks)]
        clip = FakeNoteClip(64.0, [FakeMidiNote(index + 1, 36 + index % 24, index * 0.25, 0.25) for index in range(40)]); clip.start_time = 0.0; song.tracks[0].arrangement_clips = [clip]
        return song, LiveObjectMapper(song)

    @staticmethod
    def check_like_the_host(answer, request):
        """The host's checkSnapshotAnswer rules for a windowed or focused answer."""
        window = answer["window"]; rows = answer["tracks"]; start = window.get("tracks", {}).get("from", 0)
        if "tracks" in window: assert 1 <= window["tracks"]["count"] <= request["tracks"]["count"] and len(rows) == min(window["tracks"]["count"], answer["trackCount"] - start)
        if "focus" in window: assert set(window["focus"]) <= set(request["focus"])
        focus = set(window["focus"]) if "focus" in window else None
        for position, row in enumerate(rows): assert (row.get("light") is True) == (focus is not None and start + position not in focus), position

    def test_a_spent_budget_stops_after_one_unit_even_when_the_clock_seems_to_stand_still(self):
        # A clock may not seem to move between two quick units.
        with patch.object(remote_module.time, "perf_counter", return_value=1000.0):
            spent = remote_module._ReadBudget(0); roomy = remote_module._ReadBudget(10)
            self.assertEqual([spent.room(), spent.room()], [True, False])
            self.assertEqual([roomy.room(), roomy.room(), roomy.room()], [True, True, True])

    def test_a_snapshot_window_ends_and_a_focus_goes_light_when_the_budget_is_spent(self):
        song, mapper = self.set(); mapper.read_budget_seconds = 0
        window = {"tracks": {"from": 0, "count": 3}}
        cut = mapper.snapshot(window, budgeted=True); validate_operation_payload("snapshot", "result", cut); self.check_like_the_host(cut, window)
        self.assertEqual((cut["window"]["tracks"], len(cut["tracks"])), ({"from": 0, "count": 1}, 1))
        focused = {"focus": [0, 1, 2]}
        cut = mapper.snapshot(focused, budgeted=True); self.check_like_the_host(cut, focused)
        self.assertEqual((cut["window"]["focus"], [row.get("light") is True for row in cut["tracks"]]), ([0], [False, True, True]))
        self.assertEqual(len(mapper.snapshot(focused, budgeted=True)["arrangement"]["clips"]), 1)
        # Unbudgeted (Live's own checks) and with room, it's all there; without arguments, always the whole Set.
        self.assertEqual(mapper.snapshot(focused)["window"]["focus"], [0, 1, 2])
        self.assertNotIn("window", mapper.snapshot(None, budgeted=True))
        mapper.read_budget_seconds = 10
        self.assertEqual(mapper.snapshot(window, budgeted=True)["window"]["tracks"], {"from": 0, "count": 3})
        # The wire's snapshots are budgeted.
        bridge = immediate_bridge(song); bridge.mapper.read_budget_seconds = 0
        self.assertEqual(bridge._dispatch_with_holder("snapshot", {"args": window}, {})["window"]["tracks"]["count"], 1)

    def test_the_sets_devices_page_track_by_track_and_notes_and_parameters_by_index(self):
        song, mapper = self.set(); fields = ["parentRef", "name", "className", "chainList"]
        whole = mapper.discover("device", 100000, None, None, None, fields)["items"]
        mapper.read_budget_seconds = 0
        items, pages = read_all(mapper, "device", fields=fields)
        self.assertEqual((items, pages), (whole, len(whole)))
        mapper.read_budget_seconds = 10
        items, pages = read_all(mapper, "device", fields=fields, limit=4)
        self.assertEqual((items, pages), (whole, 7))
        self.assertEqual(read_all(mapper, "device", fields=fields, filters={"className": "Operator"})[0], [item for item in whole if item["className"] == "Operator"])
        # A cursor holds its place in the Set's tracks as they were: another track ends it.
        first = mapper.discover("device", 4, None, None, None, fields, budgeted=True)
        song.tracks.append(lean_track("Late"))
        with self.assertRaisesRegex(ValueError, "invalid discovery cursor"): mapper.discover("device", 4, first["nextCursor"], None, None, fields, budgeted=True)
        # Whole rows (no fields asked) stop where the budget does.
        mapper.read_budget_seconds = 0; page = mapper.discover("device", 100, None, None, None, None, budgeted=True)
        self.assertEqual(len(page["items"]), 1); self.assertIn("parameters", page["items"][0]); self.assertTrue(page["truncated"])
        # A clip's notes and a device's parameters, by index.
        clip_ref = f"{mapper.refs.epoch}:arrangement_clip:0:0"; notes = mapper.discover("note", 100000, None, clip_ref)["items"]
        self.assertEqual(read_all(mapper, "note", clip_ref), (notes, 40))
        mapper.read_budget_seconds = 10
        self.assertEqual(read_all(mapper, "note", clip_ref, limit=16), (notes, 3))
        first = mapper.discover("note", 16, None, clip_ref, None, None, budgeted=True); song.tracks[0].arrangement_clips[0].stored.pop(40)
        with self.assertRaisesRegex(ValueError, "invalid discovery cursor"): mapper.discover("note", 16, first["nextCursor"], clip_ref, None, None, budgeted=True)
        operator = mapper.snapshot()["tracks"][0]["devices"][0]
        self.assertEqual(read_all(mapper, "parameter", operator["ref"], limit=50), (operator["parameters"], 4))
        mapper.read_budget_seconds = 0
        self.assertEqual(read_all(mapper, "parameter", operator["ref"])[0], operator["parameters"])

    def test_a_page_of_whole_tracks_stops_at_its_budget(self):
        song, mapper = self.set(); mapper.read_budget_seconds = 0
        whole = [mapper._whole_track_row(index) for index in range(3)]
        items, pages = read_all(mapper, "track")
        self.assertEqual((items, pages), (whole, 3))
        mapper.read_budget_seconds = 10
        self.assertEqual(read_all(mapper, "track", fields=["name"])[1], 1)


class SelectionWithoutRowsTests(unittest.TestCase):
    """C: a snapshot's selection names what's selected by identity on its track, without building
    the track's whole row (every device's parameters)."""

    def test_the_selection_reads_only_the_selected_parameters_device(self):
        song = FakeSong(); song.tracks = [lean_track("Synth"), lean_track("Bass")]; track = song.tracks[1]
        utility = track.devices[4].chains[1].devices[0].chains[0].devices[0]; chosen = utility._parameters[3]
        song.view = types.SimpleNamespace(selected_track=track, selected_scene=None, highlighted_clip_slot=track.clip_slots[2], detail_clip=None, selected_parameter=chosen, selected_chain=track.devices[4].chains[1])
        track.view = types.SimpleNamespace(selected_device=utility)
        mapper = LiveObjectMapper(song); whole = mapper.snapshot()
        expected = mapper._selection_row(whole["tracks"], whole["scenes"])
        self.assertEqual(expected["parameterRef"], whole["tracks"][1]["devices"][4]["chains"][1]["devices"][0]["chains"][0]["devices"][0]["parameters"][3]["ref"])
        LeanDevice.parameter_reads = 0
        self.assertEqual(mapper._selection_row_targeted(), expected)
        # Without canonical_parent to name the owner, every device on the selected track is looked at, not built.
        self.assertLessEqual(LeanDevice.parameter_reads, 9)
        answer = mapper.snapshot({"focus": [0], "parts": ["selection"]}, budgeted=True)
        self.assertEqual(answer["selection"], expected)


class CreationScopeTests(unittest.TestCase):
    """D: a creation's rollback check reads the contents of the tracks it names, not the Set's."""

    def song(self, tracks=20):
        song = FakeSong(); song.tracks = [lean_track(f"Track {index + 1}") for index in range(tracks)]
        def duplicate_track(index): song.tracks.insert(index + 1, lean_track(song.tracks[index].name + " copy"))
        song.duplicate_track = duplicate_track
        return song

    def test_a_track_duplicate_reads_below_the_track_it_names_alone(self):
        song = self.song(); bridge = immediate_bridge(song, provenance="real-live"); mapper = bridge.mapper
        row = mapper.discover("track", 20, None, None, None, ["objectIdentity"])["items"][7]
        args = {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}
        counter = ReadCounter(song.tracks)
        made = mutate_through(bridge, "track.duplicate", args, "duplicate-scope-0001", transaction="transaction-scope")
        self.assertEqual(set(counter.reads), {7}); self.assertEqual(song.tracks[8].name, "Track 8 copy")
        # Its transaction still undoes it exactly.
        delete = {"ref": made["ref"], "expectedObjectIdentity": made["objectIdentity"], "expectedStructureRevision": mapper._structure_revision()}
        self.assertEqual(bridge._dispatch_with_holder("mutate", {"operation": "track.delete", "transactionId": "transaction-scope", "idempotencyKey": "undo-scope-0001", "ownershipToken": made["ownershipToken"], "args": delete}, {}), {"deleted": made["ref"]})
        self.assertEqual(len(song.tracks), 20)

    def test_the_topology_details_only_the_scope(self):
        song = self.song(4); mapper = LiveObjectMapper(song)
        whole = json.loads(mapper._creation_topology()); scoped = json.loads(mapper._creation_topology([2])); bare = json.loads(mapper._creation_topology(()))
        self.assertEqual([sorted(row) for row in whole["tracks"]], [["arrangement", "devices", "identity", "slots"]] * 4)
        self.assertEqual([sorted(row) for row in scoped["tracks"]], [["identity"], ["identity"], ["arrangement", "devices", "identity", "slots"], ["identity"]])
        self.assertEqual(scoped["tracks"][2], whole["tracks"][2]); self.assertEqual([row["identity"] for row in bare["tracks"]], [row["identity"] for row in whole["tracks"]])
        self.assertEqual(mapper._creation_scope("clip.duplicate", {"ref": f"{mapper.refs.epoch}:clip:1:0", "targetTrackRef": f"{mapper.refs.epoch}:track:3"}), [1, 3])
        self.assertIsNone(mapper._creation_scope("scene.capture", {}))


class WatchedSong(ListenSong):
    LISTENABLE = ListenSong.LISTENABLE | {"return_tracks"}


class FlatChangeTests(unittest.TestCase):
    """D: an ordinary change binds what it depends on, and the structure revision is kept between
    requests while Live's listeners watch it."""

    def watched_song(self, tracks=6):
        song = WatchedSong(); song.return_tracks = []; song.scenes = [ListenScene(f"Scene {index + 1}") for index in range(3)]
        song.tracks = [ListenTrack() for _ in range(tracks)]
        for index, track in enumerate(song.tracks): track.name = f"Track {index + 1}"
        return song

    def test_the_structure_revision_is_kept_until_a_listener_or_the_ticks_drop_it(self):
        song = self.watched_song(); mapper = LiveObjectMapper(song); reads = []
        entries = LiveObjectMapper._track_entries
        with patch.object(LiveObjectMapper, "_track_entries", lambda self, kinds=True: reads.append(kinds) or entries(self, kinds)):
            first = mapper._structure_revision(); self.assertEqual(len(reads), 1)
            self.assertEqual(mapper._structure_revision(), first); self.assertEqual(len(reads), 1, "kept: nothing read")
            song.tracks[2].name = "Bass"
            renamed = mapper._structure_revision(); self.assertNotEqual(renamed, first); self.assertEqual(len(reads), 2)
            song.tracks = song.tracks + [ListenTrack()]
            added = mapper._structure_revision(); self.assertNotEqual(added, renamed)
            song.tracks[6].name = "New"
            self.assertNotEqual(mapper._structure_revision(), added, "a new track's name is watched too")
            count = len(reads)
            for _ in range(LiveObjectMapper.STRUCTURE_HOLD_TICKS): mapper.structure_tick()
            mapper._structure_revision(); self.assertEqual(len(reads), count + 1, "and the ticks age it")
        self.assertGreater(song.listening(), 0)
        mapper.invoke("session.reconnect", {})
        self.assertEqual((song.listening(), sum(track.listening() for track in song.tracks)), (0, 0)); self.assertIsNone(mapper._structure_held)
        # A Live that can't be watched is read every time.
        plain = LiveObjectMapper(FakeSong()); plain._structure_revision(); self.assertIsNone(plain._structure_held)

    def test_a_change_drops_the_kept_revision_even_before_live_tells_its_listeners(self):
        # On real Live a rename's name listener can fire after the next request in the same tick: a preview
        # then read the old name from the kept revision, its apply the new one, and the change was refused.
        song = self.watched_song(); mapper = LiveObjectMapper(song)
        row = mapper.snapshot()["tracks"][2]; before = mapper._structure_revision()
        with patch.object(ListenTrack, "notify", lambda self, name: None):
            mapper.invoke("track.rename", {"ref": row["ref"], "name": "Bass", "expectedName": row["name"], "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", row["ref"])})
            after = mapper._structure_revision()
        self.assertNotEqual(after, before, "the revision has the new name though no listener told")
        mapper._structure_held = None
        self.assertEqual(after, mapper._structure_revision(), "and it's what Live holds now")

    def test_a_rename_binds_the_track_not_what_it_holds(self):
        song = FakeSong(); song.tracks = [lean_track("Synth"), lean_track("Bass")]; mapper = LiveObjectMapper(song); row = mapper.snapshot()["tracks"][1]
        args = {"ref": row["ref"], "name": "Sub", "expectedName": "Bass", "expectedObjectIdentity": row["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", row["ref"])}
        digest = _authority_state_digest(mapper, args, "track.rename")
        LeanDevice.parameter_reads = 0
        song.tracks[1].devices.append(LeanDevice("Utility", "StereoGain", 20)); song.tracks[1].devices[0]._parameters[0].value = 0.9
        self.assertEqual(_authority_state_digest(mapper, args, "track.rename"), digest, "what the track holds isn't what a rename depends on")
        self.assertEqual(LeanDevice.parameter_reads, 0)
        # Nor is the rest of its state, the Set's other tracks or its scenes: its identity and name at its place are.
        song.tracks[1].arm = True; song.tracks[0].name = "Lead"; song.scenes[0].name = "Intro"
        self.assertEqual(_authority_state_digest(mapper, args, "track.rename"), digest, "only its identity and name are")
        song.tracks[1].name = "Bass 2"
        self.assertNotEqual(_authority_state_digest(mapper, args, "track.rename"), digest, "its name is")
        song.tracks[1].name = "Bass"
        self.assertEqual(mapper.invoke("track.rename", args), {"renamed": row["ref"], "name": "Sub"})
        # A device rename's authority, from the light walk, is the one whole rows gave.
        device = mapper.snapshot()["tracks"][0]["devices"][4]["chains"][1]["devices"][0]
        owner = mapper.snapshot()["tracks"][0]["devices"][4]["chains"][1]
        expected = {"ref": device["ref"], "objectIdentity": device["objectIdentity"], "trackRef": mapper.snapshot()["tracks"][0]["ref"], "trackIdentity": mapper.snapshot()["tracks"][0]["objectIdentity"], "ownerRef": owner["ref"], "ownerIdentity": owner["objectIdentity"], "siblings": [{"ref": item["ref"], "objectIdentity": item["objectIdentity"]} for item in owner["devices"]]}
        self.assertEqual(mapper._rename_authority_revision("device", device["ref"]), hashlib.sha256(mapper._bounded_canonical(expected).encode()).hexdigest())


class LeanNoteClip:
    """A long Arrangement MIDI clip: notes as Live 12 hands them over (a vector of note objects)."""
    is_audio_clip = False

    def __init__(self, notes, length=1000.0):
        self.name = "Long MIDI"; self.length = length; self.start_time = 0.0; self.looping = False; self.muted = False
        self.notes = [FakeMidiNote(index + 1, 36 + index % 48, index * length / notes, 0.25) for index in range(notes)]

    def get_all_notes_extended(self): return list(self.notes)
    def add_new_notes(self, notes): pass


class WatchedLeanTrack(Listenable, FakeTrack):
    LISTENABLE = frozenset({"name"})


def measured_set(tracks=200, notes=20000, watched=False):
    """The Set measured on real Live: tracks copied from one template (Operator, EQ Eight,
    Compressor, Reverb, an Audio Effect Rack with two chains, one holding a nested rack), about
    470 parameters a track; track 0 holds a 1000-beat Arrangement clip of `notes` notes."""
    song = WatchedSong() if watched else FakeSong(); song.return_tracks = []
    def make(name):
        track = lean_track(name)
        if watched: track.__class__ = WatchedLeanTrack
        return track
    song.tracks = [make(f"Track {index + 1}") for index in range(tracks)]; song.scenes = [(ListenScene if watched else FakeScene)(f"Scene {index + 1}") for index in range(8)]
    song.tracks[0].arrangement_clips = [LeanNoteClip(notes)]
    def duplicate_track(index): song.tracks = song.tracks[:index + 1] + [make(song.tracks[index].name + " copy")] + song.tracks[index + 1:]
    song.duplicate_track = duplicate_track
    return song


class MeasuredSetBenchmarkTests(unittest.TestCase):
    """E: the reads and changes measured on real Live, on a fake of the same Set. Prints what each
    costs (total, the slowest single request: what Live's UI feels, and how many requests); asserts
    what each reads and that no budgeted request runs away."""

    def paged(self, mapper, kind, parent=None, fields=None):
        items, pages, slowest, total, cursor = [], 0, 0.0, 0.0, None
        while True:
            started = time.perf_counter(); page = mapper.discover(kind, 100000, cursor, parent, None, fields, budgeted=True); elapsed = time.perf_counter() - started
            items += page["items"]; pages += 1; slowest = max(slowest, elapsed); total += elapsed; cursor = page.get("nextCursor")
            if not cursor: return items, total * 1000, slowest * 1000, pages

    def test_the_measured_sets_reads_and_changes(self):
        song = measured_set(); mapper = LiveObjectMapper(song); report = []
        line = lambda label, total, slowest=None, requests=1: report.append(f"    {label:58} {total:8.1f} ms" + (f"  slowest {slowest:6.1f} ms  {requests:4d} requests" if slowest is not None else ""))
        tracks, total, slowest, pages = self.paged(mapper, "track", fields=["name", "kind", "mediaKind", "groupTrackRef"]); line("discover track (observation fields)", total, slowest, pages)
        self.assertEqual(len(tracks), 200)
        LeanDevice.parameter_reads = 0
        devices, total, slowest, pages = self.paged(mapper, "device", fields=["parentRef", "name", "className", "chainList"]); line("discover device, Set-wide (observation fields)", total, slowest, pages)
        self.assertEqual((len(devices), LeanDevice.parameter_reads), (1800, 0))
        tree_fields = ["parentRef", "name", "className", "canHaveChains", "canHaveDrumPads", "chainList", "deviceType"]; started = time.perf_counter(); level = self.paged(mapper, "device", tracks[0]["ref"], tree_fields)[0]; requests = 1
        chains = [chain["ref"] for device in level for chain in device.get("chainList", [])]
        while chains:
            requests += len(chains); chains = [item["ref"] for chain in chains for device in self.paged(mapper, "device", chain, tree_fields)[0] for item in device.get("chainList", [])]
        line(f"one track's device tree, level by level ({requests} requests)", (time.perf_counter() - started) * 1000)
        LeanDevice.parameter_reads = 0
        parameters, total, slowest, pages = self.paged(mapper, "parameter", devices[0]["ref"]); line("discover parameter, parent Operator", total, slowest, pages)
        self.assertEqual((len(parameters), LeanDevice.parameter_reads), (195, 1))
        # The two budgeted reads below each run twice, and the faster pass counts: a shared runner can pause
        # a request (another job, Python's collector) well past its budget, but rarely in both passes, while
        # a read that ignores its budget overruns in both.
        notes, total, slowest_notes, pages = min((self.paged(mapper, "note", f"{mapper.refs.epoch}:arrangement_clip:0:0") for _ in range(2)), key=lambda run: run[2]); line("discover note, parent the Arrangement clip (faster of 2)", total, slowest_notes, pages)
        self.assertEqual(len(notes), 20000)
        started = time.perf_counter(); whole = mapper.snapshot(); line("snapshot without arguments (the whole Set, unbudgeted)", (time.perf_counter() - started) * 1000)
        self.assertNotIn("notes", whole["arrangement"]["clips"][0]); self.assertEqual(whole["arrangement"]["clips"][0]["noteCount"], 20000)
        def windows():
            start, requests, slowest, total = 0, 0, 0.0, 0.0
            while start < 200:
                began = time.perf_counter(); page = mapper.snapshot({"tracks": {"from": start, "count": 16}, "parts": ["tracks", "arrangement"]}, budgeted=True); elapsed = time.perf_counter() - began
                start += page["window"]["tracks"]["count"]; requests += 1; slowest = max(slowest, elapsed); total += elapsed
            return total, slowest, requests
        total, slowest_window, requests = min((windows() for _ in range(2)), key=lambda run: run[1])
        line("snapshot, the whole Set in windows of 16 (faster of 2)", total * 1000, slowest_window * 1000, requests)
        # No budgeted request runs away: its budget, plus one unit at most (a whole track row here).
        self.assertLess(max(slowest_notes, slowest_window * 1000), 150)
        for size in (20, 200):
            bridge = immediate_bridge(measured_set(size, 10), provenance="real-live"); row = bridge.mapper.discover("track", 1)["items"][0]
            args = {"ref": row["ref"], "expectedObjectIdentity": row["objectIdentity"], "expectedStructureRevision": bridge.mapper._structure_revision()}
            holder = {}; started = time.perf_counter()
            pre = bridge._dispatch_with_holder("preflight", {"operation": "track.duplicate", "args": args, "transactionId": "transaction-bench"}, holder)
            prepared = bridge._dispatch_with_holder("prepare", {"operation": "track.duplicate", "args": args, "transactionId": "transaction-bench", "preflightToken": pre["preflightToken"], "confirmation": pre["confirmation"], "idempotencyKey": "bench-duplicate-0001"}, holder)
            bridge._dispatch_with_holder("invoke", {"operation": "track.duplicate", "args": args, "transactionId": "transaction-bench", "authorityToken": prepared["authorityToken"]}, holder)
            line(f"track.duplicate, preflight/prepare/invoke, {size} tracks", (time.perf_counter() - started) * 1000)
        for watched in (False, True):
            bridge = immediate_bridge(measured_set(200, 10, watched), provenance="real-live"); mapper = bridge.mapper; track = mapper.discover("track", 200)["items"][100]
            rows = []; built = LiveObjectMapper._track_row
            with patch.object(LiveObjectMapper, "_track_row", lambda self, *row_args: rows.append(row_args[2]) or built(self, *row_args)):
                for attempt in range(2):
                    args = {"ref": track["ref"], "name": f"Renamed {attempt}", "expectedName": mapper.song.tracks[100].name, "expectedObjectIdentity": track["objectIdentity"], "expectedAuthorityRevision": mapper._rename_authority_revision("track", track["ref"])}
                    started = time.perf_counter()
                    digest = mapper.invoke("authority.digest", {"operation": "track.rename", "args": args})["stateDigest"]
                    bridge._dispatch_with_holder("mutate", {"operation": "track.rename", "transactionId": "transaction-rename", "idempotencyKey": f"bench-rename-{watched:d}{attempt}", "stateDigest": digest, "args": args}, {})
                    elapsed = (time.perf_counter() - started) * 1000
            self.assertEqual(mapper.song.tracks[100].name, "Renamed 1"); self.assertEqual(rows, [], "a rename builds no track's whole row")
            line(f"track.rename, one mutate, 200 tracks ({'watched' if watched else 'unwatched'} structure)", elapsed)
        print("\n  the measured Set (200 tracks, 1800 devices, a 20000-note Arrangement clip), Remote Script side:\n" + "\n".join(report))


class OneObjectChangeTests(unittest.TestCase):
    """A parameter, mixer or rename change reads what it touches, however big the Set and its tracks:
    the parameter with its device's and track's identities, the track's mixer, the track's or scene's
    name. Its checks stay: each object's identity at its place, its expected values, and a digest of
    what it names. Discovering one object by its ref reads that object, as the whole list has it."""

    @staticmethod
    def counted(work):
        """What work returns, and how many attributes it read off the Set's objects (Live's are LOM reads)."""
        reads = [0]
        def counting(cls):
            original = cls.__getattribute__
            def getattribute(self, name):
                if not name.startswith("__"): reads[0] += 1
                return original(self, name)
            return getattribute
        patches = [patch.object(cls, "__getattribute__", counting(cls)) for cls in (LeanParameter, LeanChain, LeanDevice, LeanPad, FakeTrack, FakeMixerDevice, FakeSlot, FakeParameter, FakeSong, FakeScene)]
        for item in patches: item.start()
        try: result = work()
        finally:
            for item in patches: item.stop()
        return result, reads[0]

    @staticmethod
    def one(mapper, kind, reference, fields=None, parent=None):
        """The object a ref names, discovered by its ref as the host reads it."""
        page = mapper.discover(kind, 1, None, parent, {"ref": reference}, fields, budgeted=True)
        validate_operation_payload("discover", "result", page)
        return page["items"]

    def test_a_parameter_change_reads_its_parameter_its_device_and_its_track(self):
        costs = []
        for size in (20, 200):
            mapper = LiveObjectMapper(measured_set(size, 10), provenance="real-live"); epoch = mapper.refs.epoch
            parameter = f"{epoch}:parameter:{epoch}:device:10:0:3"; whole = mapper._whole_track_row(10); device = whole["devices"][0]; row = device["parameters"][3]
            # Fenced on the parameter, its device and its track, and none of the device's other parameters.
            args = {"ref": parameter, "value": 0.25, "expectedRevision": row["revision"], "expectedObjectIdentity": row["objectIdentity"], "expectedOwnerRef": device["ref"], "expectedOwnerIdentity": device["objectIdentity"], "expectedTrackRef": whole["ref"], "expectedTrackIdentity": whole["objectIdentity"], "expectedSiblings": []}
            validate_operation_payload("device.parameter.set", "request", args)
            LeanDevice.parameter_reads = 0
            with patch.object(LiveObjectMapper, "_whole_track_row", side_effect=AssertionError("a parameter change builds no whole track row")):
                _, digesting = self.counted(lambda: _authority_state_digest(mapper, args, "device.parameter.set"))
                changed, changing = self.counted(lambda: mapper.invoke("device.parameter.set", args, "transaction-one-parameter"))
                self.assertEqual(changed["value"], 0.25); self.assertLessEqual(LeanDevice.parameter_reads, 2, "Operator's parameter list, no other device's")
                (read, reading) = self.counted(lambda: (self.one(mapper, "parameter", parameter, parent=device["ref"]), self.one(mapper, "device", device["ref"], ["ref", "parentRef", "objectIdentity", "name", "kind", "enabled"]), self.one(mapper, "track", whole["ref"], ["ref", "objectIdentity", "name", "kind"])))
            self.assertEqual(read[0], [mapper._whole_track_row(10)["devices"][0]["parameters"][3]], "the row the list has")
            self.assertEqual(read[1], [{"ref": device["ref"], "parentRef": whole["ref"], "objectIdentity": device["objectIdentity"], "name": "Operator", "kind": "device", "enabled": True}])
            self.assertEqual(read[2], [{"ref": whole["ref"], "parentRef": whole["parentRef"], "objectIdentity": whole["objectIdentity"], "name": whole["name"], "kind": "regular"}])
            costs.append((digesting, changing, reading))
        self.assertEqual(costs[0], costs[1], "as many reads on 200 tracks as on 20"); self.assertLess(sum(costs[1]), 400)
        for wrong in ({"expectedObjectIdentity": "live:other"}, {"expectedOwnerIdentity": "live:other"}, {"expectedTrackIdentity": "live:other"}, {"expectedOwnerRef": f"{epoch}:device:10:1"}):
            with self.assertRaisesRegex(ValueError, "identity or hierarchy changed"): mapper.invoke("device.parameter.set", {**args, **wrong, "expectedRevision": mapper.refs.revision(parameter)}, "transaction-one-parameter-wrong")
        # Another device at the parameter's place now: refused.
        devices = mapper.song.tracks[10].devices; devices[0], devices[1] = devices[1], devices[0]
        with self.assertRaisesRegex(ValueError, "identity or hierarchy changed"): mapper.invoke("device.parameter.set", {**args, "expectedRevision": mapper.refs.revision(parameter)}, "transaction-one-parameter-moved")
        # A nested device's parameter, through its racks' chains.
        nested = f"{epoch}:device:10:4:1:0:0:0"; row = self.one(mapper, "parameter", f"{epoch}:parameter:{nested}:2", parent=nested)[0]
        self.assertEqual((row["parentRef"], self.one(mapper, "device", nested, ["name"])), (nested, [{"ref": nested, "parentRef": f"{epoch}:chain:10:4:1:0:0", "name": "Utility"}]))

    def test_a_mixer_change_reads_its_tracks_mixer(self):
        costs = []
        for size in (20, 200):
            mapper = LiveObjectMapper(measured_set(size, 10), provenance="real-live"); epoch = mapper.refs.epoch; track = f"{epoch}:track:10"
            whole = mapper._whole_track_row(10); mixer = whole["mixer"]; state = {field: mixer.get(field) for field in ("volume", "pan", "mute", "solo", "cueVolume", "sends")}
            args = {"ref": track, "volume": 0.4, "expectedObjectIdentity": whole["objectIdentity"], "expectedVolumeIdentity": mixer["volumeIdentity"], "expectedPanIdentity": mixer["panIdentity"], "expectedCueIdentity": mixer["cueIdentity"], "expectedSendIdentities": mixer["sendIdentities"], "expectedStateRevision": hashlib.sha256(mapper._bounded_canonical(state).encode("utf-8")).hexdigest()}
            with patch.object(LiveObjectMapper, "_whole_track_row", side_effect=AssertionError("a mixer change builds no whole track row")):
                _, digesting = self.counted(lambda: _authority_state_digest(mapper, args, "mixer.set"))
                _, changing = self.counted(lambda: mapper.invoke("mixer.set", args, "transaction-one-mixer"))
                read, reading = self.counted(lambda: self.one(mapper, "track", track, ["ref", "objectIdentity", "name", "kind", "mixer"]))
            self.assertEqual(mapper.song.tracks[10].mixer_device.volume.value, 0.4)
            self.assertEqual(read, [{"ref": track, "parentRef": whole["parentRef"], "objectIdentity": whole["objectIdentity"], "name": whole["name"], "kind": "regular", "mixer": mapper._whole_track_row(10)["mixer"]}], "the mixer row a whole row has")
            costs.append((digesting, changing, reading))
        self.assertEqual(costs[0], costs[1], "as many reads on 200 tracks as on 20"); self.assertLess(sum(costs[1]), 800)
        # Another track at the ref's place now: refused.
        mapper.song.tracks[10], mapper.song.tracks[11] = mapper.song.tracks[11], mapper.song.tracks[10]
        with self.assertRaisesRegex(ValueError, "identity changed"): mapper.invoke("mixer.set", args, "transaction-one-mixer-moved")

    def test_a_rename_reads_its_tracks_or_scenes_name_and_identity(self):
        costs = []
        for size in (20, 200):
            mapper = LiveObjectMapper(measured_set(size, 10), provenance="real-live"); epoch = mapper.refs.epoch; rows = []
            for kind, reference, name in (("track", f"{epoch}:track:10", "Track 11"), ("scene", f"{epoch}:scene:2", "Scene 3")):
                identity = mapper._positional_identity(reference)
                args = {"ref": reference, "name": f"{name} renamed", "expectedName": name, "expectedObjectIdentity": identity, "expectedAuthorityRevision": hashlib.sha256(mapper._bounded_canonical({"ref": reference, "objectIdentity": identity, "name": name}).encode("utf-8")).hexdigest()}
                with patch.object(LiveObjectMapper, "_structure_revision", side_effect=AssertionError("a rename reads no Set structure")), patch.object(LiveObjectMapper, "_whole_track_row", side_effect=AssertionError("a rename builds no whole track row")):
                    _, digesting = self.counted(lambda: _authority_state_digest(mapper, args, f"{kind}.rename"))
                    renamed, renaming = self.counted(lambda: mapper.invoke(f"{kind}.rename", args))
                    read, reading = self.counted(lambda: self.one(mapper, kind, reference, ["ref", "objectIdentity", "name"]))
                self.assertEqual(renamed["name"], f"{name} renamed")
                self.assertEqual([{key: value for key, value in item.items() if key != "parentRef"} for item in read], [{"ref": reference, "objectIdentity": identity, "name": f"{name} renamed"}])
                rows.append((digesting, renaming, reading))
            costs.append(rows)
        self.assertEqual(costs[0], costs[1], "as many reads on 200 tracks as on 20"); self.assertLess(sum(sum(item) for item in costs[1]), 200)
        # Its name changed by hand since the preview: refused.
        mapper.song.tracks[10].name = "By hand"
        with self.assertRaisesRegex(ValueError, "changed since preview"): mapper.invoke("track.rename", {"ref": f"{epoch}:track:10", "name": "Again", "expectedName": "Track 11 renamed", "expectedObjectIdentity": mapper._positional_identity(f"{epoch}:track:10"), "expectedAuthorityRevision": mapper._rename_authority_revision("track", f"{epoch}:track:10")})

    def test_one_object_by_its_ref_is_the_whole_lists_page_filtered_to_it(self):
        mapper = LiveObjectMapper(measured_set(12, 10), provenance="real-live"); epoch = mapper.refs.epoch
        def listed(kind, reference, fields=None, parent=None):
            page = mapper.discover(kind, 100000, None, parent, {"ref": reference, "name": None} if False else None, fields, budgeted=False)
            return [item for item in page["items"] if item["ref"] == reference]
        cases = [("track", f"{epoch}:track:3", ["ref", "objectIdentity", "name", "kind"], None), ("track", f"{epoch}:track:3", ["ref", "mixer"], None), ("scene", f"{epoch}:scene:1", None, None),
                 ("device", f"{epoch}:device:3:4", ["ref", "parentRef", "name", "chainList"], None), ("device", f"{epoch}:device:3:4:0:1", ["ref", "parentRef", "name"], f"{epoch}:chain:3:4:0"),
                 ("parameter", f"{epoch}:parameter:{epoch}:device:3:2:7", None, f"{epoch}:device:3:2")]
        for kind, reference, fields, parent in cases:
            self.assertEqual(self.one(mapper, kind, reference, fields, parent), listed(kind, reference, fields, parent), reference)
        # A ref of another kind, a stale place or a parent not its own: nothing.
        self.assertEqual(self.one(mapper, "return_track", f"{epoch}:track:3"), [])
        self.assertEqual(self.one(mapper, "track", f"{epoch}:track:99"), [])
        self.assertEqual(self.one(mapper, "device", f"{epoch}:device:3:4:0:1", parent=f"{epoch}:track:3"), [])
