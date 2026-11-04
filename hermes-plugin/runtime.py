"""Kumi's runtime for Hermes: the `kumi` setup installed, `kumi serve --stdio` as a child process, MCP over its pipes.

`kumi serve` is a headless mode of Kumi's own release binary (RESEARCH.md C2). This module finds the copy setup installed
in this profile's plugin data, starts it with a scrubbed environment, checks it's the pinned runtime before anything else,
and calls its tools. A profile has one runtime in a process, owned by the chat that started it. It stops when that chat
ends, when the plugin unloads, or when Hermes exits: its stdin closes and it settles and exits by itself.

Nothing here runs at import or in register(). Standard library only, Python 3.11.
"""
from __future__ import annotations

import contextvars
from dataclasses import dataclass, field
import itertools
import json
import logging
import os
from pathlib import Path
import subprocess
import threading
from typing import Any, Callable, Mapping

from .setup import host_target

log = logging.getLogger(__name__)

MCP_PROTOCOL = "2025-11-25"
SERVER_NAME = "kumi-serve"
RUNTIME_PROTOCOL = "kumi-serve/1"
CLIENT_INFO = {"name": "kumi-hermes", "version": "1"}
# The snapshots `kumi serve` advertises as resources (C2), each named as its C3 file and its C5 event.
RESOURCES = {"kumi://status": "status", "kumi://focus": "focus", "kumi://history": "history", "kumi://map": "map"}
# What `kumi serve` gets of Hermes's environment: what a program needs to start and to find the user's folders on each
# system. Never a key, a token or Hermes's own settings (C2, catalog rule 11).
LAUNCH_VARIABLES = frozenset({
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "LC_ALL", "LC_CTYPE", "LC_MESSAGES", "TMPDIR", "TEMP", "TMP",
    "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "APPDATA", "LOCALAPPDATA",
    "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)",
})


class Unavailable(Exception):
    """Kumi's runtime can't be used: `code` says why in a word tools pass on (setup_needed, wrong_runtime, stopped,
    timeout, protocol), `reason` in a sentence."""

    def __init__(self, code: str, reason: str):
        super().__init__(reason)
        self.code = code
        self.reason = reason


class Refused(Exception):
    """A tool call `kumi serve` answered with an error: it never reached the DAW (W1-REGISTRY: `OpError`)."""

    def __init__(self, code: str, reason: str, owner: dict | None = None):
        super().__init__(reason)
        self.code = code
        self.reason = reason
        self.owner = owner

    def as_json(self) -> dict:
        error = {"code": self.code, "reason": self.reason}
        if self.owner:
            error["owner"] = self.owner
        return {"error": error}


@dataclass(frozen=True)
class Install:
    """The runtime setup installed: its version and target, the `kumi` to start, and Kumi's transport home."""
    version: str
    target: str
    kumi: Path
    kumi_home: Path


def installed(data_dir: Path, pinned: str | None, target: str | None = None) -> Install:
    """The runtime setup installed in `data_dir` (this profile's plugin data), from runtime/current.json, which setup
    writes last: only one of the pinned version, for this computer, whose `kumi` is an executable inside `data_dir`.
    Anything else raises Unavailable("setup_needed"): setup repairs it, never this."""
    target = target or host_target()
    if not pinned:
        raise Unavailable("setup_needed", "This plugin pins no Kumi release yet.")
    try:
        receipt = json.loads((data_dir / "runtime" / "current.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise Unavailable("setup_needed", "Kumi isn't set up in this Hermes profile yet.") from None
    except (OSError, ValueError):
        raise Unavailable("setup_needed", "Kumi's setup record can't be read: set Kumi up again.") from None
    if not isinstance(receipt, dict) or receipt.get("schema") != 1:
        raise Unavailable("setup_needed", "Kumi's setup record is from another plugin version: set Kumi up again.")
    version, kumi = receipt.get("version"), receipt.get("kumi")
    if version != pinned:
        raise Unavailable("setup_needed", f"Kumi {version} is set up, and this plugin needs {pinned}: set Kumi up again.")
    if receipt.get("target") != target:
        raise Unavailable("setup_needed", f"Kumi is set up for {receipt.get('target')}, not this computer ({target}).")
    root = data_dir.resolve()
    program = (data_dir / kumi).resolve() if isinstance(kumi, str) and kumi else None
    if program is None or root not in program.parents or not program.is_file() or not os.access(program, os.X_OK):
        raise Unavailable("setup_needed", "Kumi's runtime is missing or damaged: set Kumi up again.")
    home = receipt.get("kumi_home")
    return Install(version, target, program, Path(home) if isinstance(home, str) and home else data_dir / "kumi-home")


def launch_environment(environ: Mapping[str, str], windows: bool = os.name == "nt") -> dict[str, str]:
    """The environment `kumi serve` starts with: the launch variables of `environ`, and no update check (rule 3)."""
    kept = {key: value for key, value in environ.items() if (key.upper() if windows else key) in LAUNCH_VARIABLES}
    kept["KUMI_NO_UPDATE_CHECK"] = "1"
    return kept


def serve_argv(install: Install, state_dir: Path, daw: str) -> list[str]:
    return [str(install.kumi), "serve", "--stdio", "--state-dir", str(state_dir), "--kumi-home", str(install.kumi_home),
            "--daw", daw]


def _thread(target: Callable[..., Any], name: str, *args: Any) -> threading.Thread:
    """A daemon thread that runs under its starter's context variables: Hermes keeps the active profile in them
    (agent.memory_provider.spawn_context_thread does the same), and a thread started without them lands on another."""
    context = contextvars.copy_context()
    return threading.Thread(target=context.run, args=(target, *args), name=name, daemon=True)


@dataclass
class _Pending:
    done: threading.Event = field(default_factory=threading.Event)
    result: Any = None
    error: dict | None = None
    gone: str | None = None


class Client:
    """MCP over a child's stdin and stdout (the stdio transport: one JSON-RPC message a line). Requests are answered by
    id; notifications go to `on_notification` on the reader thread. When the child exits, every waiting request fails
    with Unavailable("stopped"), and nothing is sent again."""

    def __init__(self, argv: list[str], env: dict[str, str], *, cwd: str | None = None,
                 on_notification: Callable[[str, dict], None] | None = None,
                 on_exit: Callable[[str], None] | None = None):
        flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        self._process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         env=env, cwd=cwd, close_fds=True, creationflags=flags)
        self._ids = itertools.count(1)
        self._pending: dict[int, _Pending] = {}
        self._lock = threading.Lock()
        self._writing = threading.Lock()
        self._gone: str | None = None
        self._on_notification = on_notification
        self._on_exit = on_exit
        _thread(self._read, "kumi-serve-stdout").start()
        _thread(self._log_stderr, "kumi-serve-stderr").start()

    @property
    def pid(self) -> int:
        return self._process.pid

    @property
    def running(self) -> bool:
        return self._gone is None

    def request(self, method: str, params: dict | None = None, *, timeout: float) -> Any:
        """Send a request and wait for its answer. Past `timeout`, the request is cancelled (notifications/cancelled)
        and Unavailable("timeout") raised: whatever it did is unknown, and it's never sent again."""
        pending = _Pending()
        with self._lock:
            if self._gone is not None:
                raise Unavailable("stopped", self._gone)
            ident = next(self._ids)
            self._pending[ident] = pending
        try:
            self._send({"jsonrpc": "2.0", "id": ident, "method": method, "params": params or {}})
            if not pending.done.wait(timeout):
                self.notify("notifications/cancelled", {"requestId": ident, "reason": "Hermes stopped waiting"})
                raise Unavailable("timeout", f"Kumi didn't answer {method} within {timeout:g} s; it was cancelled, and "
                                             "what it did is unknown.")
        finally:
            with self._lock:
                self._pending.pop(ident, None)
        if pending.gone is not None:
            raise Unavailable("stopped", pending.gone)
        if pending.error is not None:
            raise Unavailable("protocol", f"Kumi refused {method}: {pending.error.get('message') or pending.error}")
        return pending.result

    def notify(self, method: str, params: dict | None = None) -> None:
        try:
            self._send({"jsonrpc": "2.0", "method": method, "params": params or {}})
        except Unavailable:
            pass

    def close(self, timeout: float = 10.0) -> None:
        """Close the runtime's stdin: it settles what's in flight and exits. One still running after `timeout` seconds
        is terminated, then killed."""
        try:
            self._process.stdin.close()
        except OSError:
            pass
        try:
            self._process.wait(timeout)
        except subprocess.TimeoutExpired:
            log.warning("kumi serve (pid %s) didn't exit %g s after its stdin closed: stopping it", self.pid, timeout)
            self._process.terminate()
            try:
                self._process.wait(5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()

    def _send(self, message: dict) -> None:
        line = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"
        with self._writing:
            if self._gone is not None:
                raise Unavailable("stopped", self._gone)
            try:
                self._process.stdin.write(line)
                self._process.stdin.flush()
            except (OSError, ValueError):
                raise Unavailable("stopped", "Kumi's runtime stopped reading its requests.") from None

    def _read(self) -> None:
        try:
            for raw in self._process.stdout:
                try:
                    message = json.loads(raw)
                except ValueError:
                    log.warning("kumi serve wrote a line that isn't JSON: %.200r", raw)
                    continue
                if isinstance(message, dict):
                    self._receive(message)
        except (OSError, ValueError):
            pass
        finally:
            self._process.stdout.close()
            self._exited()

    def _receive(self, message: dict) -> None:
        method = message.get("method")
        if method is None:
            with self._lock:
                pending = self._pending.get(message.get("id"))
            if pending is not None:
                pending.result, pending.error = message.get("result"), message.get("error")
                pending.done.set()
        elif "id" in message:
            # A request from the runtime: Hermes's plugin offers no client features beyond answering a ping.
            if method == "ping":
                self._reply({"jsonrpc": "2.0", "id": message["id"], "result": {}})
            else:
                self._reply({"jsonrpc": "2.0", "id": message["id"],
                             "error": {"code": -32601, "message": f"{method} isn't offered by Kumi's Hermes plugin"}})
        elif self._on_notification is not None:
            try:
                self._on_notification(method, message.get("params") or {})
            except Exception:
                log.exception("Kumi's handling of the runtime's %s failed", method)

    def _reply(self, message: dict) -> None:
        try:
            self._send(message)
        except Unavailable:
            pass

    def _exited(self) -> None:
        code = self._process.poll()
        reason = "Kumi's runtime stopped" + (f" (exit {code})." if code is not None else ".")
        with self._lock:
            self._gone = reason
            waiting = list(self._pending.values())
        for pending in waiting:
            pending.gone = reason
            pending.done.set()
        with self._writing:
            try:
                self._process.stdin.close()
            except OSError:
                pass
        try:
            self._process.wait(5)  # its output ended, so it's exiting: reaped here, it leaves no zombie
        except subprocess.TimeoutExpired:
            log.warning("kumi serve (pid %s) closed its output and is still running", self.pid)
        if self._on_exit is not None:
            try:
                self._on_exit(reason)
            except Exception:
                log.exception("Kumi's handling of its runtime's exit failed")

    def _log_stderr(self) -> None:
        try:
            for raw in self._process.stderr:
                log.info("kumi serve: %s", raw.decode("utf-8", "replace").rstrip()[:2000])
        except (OSError, ValueError):
            pass
        finally:
            self._process.stderr.close()


@dataclass
class Runtime:
    """A started `kumi serve`: its client, what it runs, and the chat that owns it."""
    client: Client
    install: Install
    data_dir: Path
    daw: str
    owner_session: str | None
    info: dict = field(default_factory=dict)

    @property
    def state_dir(self) -> Path:
        return self.data_dir / "state"

    def call(self, tool: str, args: dict, host: dict, *, timeout: float) -> dict:
        """Call one of the runtime's tools (C2: C1's names without `kumi_`), with Hermes's call identity in `_host`.
        Its answer's structured content, or Refused with the runtime's error."""
        result = self.client.request("tools/call", {"name": tool, "arguments": {**args, "_host": host}}, timeout=timeout)
        content = result.get("structuredContent") if isinstance(result, dict) else None
        if not isinstance(content, dict):
            raise Unavailable("protocol", f"Kumi's {tool} answered without structured content.")
        if result.get("isError"):
            error = content.get("error") if isinstance(content.get("error"), dict) else {}
            raise Refused(str(error.get("code") or "failed"), str(error.get("reason") or f"Kumi's {tool} failed."),
                          error.get("owner") if isinstance(error.get("owner"), dict) else None)
        return content


Spawn = Callable[..., Client]


class Runtimes:
    """This process's runtimes: one per profile (its plugin-data folder), each owned by the chat that started it.
    `events(name, payload)` is told when a snapshot changes (C5: `status.changed`, `focus.changed`, …)."""

    def __init__(self, *, events: Callable[[str, dict], None] | None = None, spawn: Spawn = Client,
                 environ: Callable[[], Mapping[str, str]] = lambda: os.environ, start_timeout: float = 30.0):
        self._events = events
        self._spawn = spawn
        self._environ = environ
        self._start_timeout = start_timeout
        self._lock = threading.Lock()
        self._running: dict[Path, Runtime] = {}

    def get(self, data_dir: Path) -> Runtime | None:
        with self._lock:
            runtime = self._running.get(data_dir.resolve())
        return runtime if runtime is not None and runtime.client.running else None

    def start(self, data_dir: Path, install: Install, daw: str, session_id: str | None) -> Runtime:
        """The profile's runtime, started for `session_id` when none runs: initialized, checked to be the pinned
        runtime (its name, version and protocol, before anything else is asked of it) and subscribed to its
        snapshots. A runtime that isn't the pinned one is stopped and Unavailable("wrong_runtime") raised."""
        key = data_dir.resolve()
        with self._lock:
            running = self._running.get(key)
            if running is not None and running.client.running:
                return running
            state = data_dir / "state"
            state.mkdir(parents=True, exist_ok=True)
            install.kumi_home.mkdir(parents=True, exist_ok=True)
            holder: dict[str, Runtime] = {}
            client = self._spawn(serve_argv(install, state, daw), launch_environment(self._environ()), cwd=str(data_dir),
                                 on_notification=lambda method, params: self._notified(holder, method, params),
                                 on_exit=lambda reason: self._exited(holder))
            runtime = Runtime(client, install, data_dir, daw, session_id)
            try:
                runtime.info = self._handshake(runtime)
            except BaseException:
                client.close(timeout=5)
                raise
            holder["runtime"] = runtime
            self._running[key] = runtime
        self._emit("status.changed", {"seq": None})
        return runtime

    def stop(self, data_dir: Path) -> None:
        with self._lock:
            runtime = self._running.pop(data_dir.resolve(), None)
        if runtime is not None:
            runtime.client.close()
            self._emit("status.changed", {"seq": None})

    def stop_session(self, session_id: str | None) -> None:
        """Stop the runtimes `session_id` owns (its chat ended)."""
        with self._lock:
            owned = [key for key, runtime in self._running.items() if session_id and runtime.owner_session == session_id]
        for key in owned:
            self.stop(key)

    def stop_all(self) -> None:
        with self._lock:
            keys = list(self._running)
        for key in keys:
            self.stop(key)

    def _handshake(self, runtime: Runtime) -> dict:
        client, expected = runtime.client, runtime.install.version
        result = client.request("initialize", {"protocolVersion": MCP_PROTOCOL, "capabilities": {},
                                               "clientInfo": CLIENT_INFO}, timeout=self._start_timeout)
        info = result.get("serverInfo") if isinstance(result, dict) else None
        info = info if isinstance(info, dict) else {}
        if info.get("name") != SERVER_NAME or info.get("version") != expected:
            raise Unavailable("wrong_runtime", f"The runtime in Kumi's install is {info.get('name')} "
                                               f"{info.get('version')}, not {SERVER_NAME} {expected}: set Kumi up again.")
        if result.get("protocolVersion") != MCP_PROTOCOL:
            raise Unavailable("wrong_runtime", f"Kumi's runtime speaks MCP {result.get('protocolVersion')}, not "
                                               f"{MCP_PROTOCOL}: set Kumi up again.")
        client.notify("notifications/initialized")
        status = runtime.call("status", {}, {"profile_id": "", "session_id": runtime.owner_session or "",
                                             "tool_call_id": "", "request_id": "handshake"}, timeout=self._start_timeout)
        protocol = status.get("runtime", {}).get("protocol") if isinstance(status.get("runtime"), dict) else None
        if protocol != RUNTIME_PROTOCOL:
            raise Unavailable("wrong_runtime", f"Kumi's runtime speaks {protocol}, not {RUNTIME_PROTOCOL}: set Kumi up "
                                               "again.")
        if (result.get("capabilities") or {}).get("resources", {}).get("subscribe"):
            for uri in RESOURCES:
                client.request("resources/subscribe", {"uri": uri}, timeout=self._start_timeout)
        return info

    def _notified(self, holder: dict[str, Runtime], method: str, params: dict) -> None:
        runtime = holder.get("runtime")
        if runtime is None or method != "notifications/resources/updated":
            return
        name = RESOURCES.get(params.get("uri"))
        if name == "map":
            revision = _read_text(runtime.state_dir / "map" / "current")
            snapshot = _read_json(runtime.state_dir / "map" / f"{revision}.json") if revision else None
            self._emit("map.changed", {"revision": revision, "seq": (snapshot or {}).get("seq")})
        elif name is not None:
            snapshot = _read_json(runtime.state_dir / f"{name}.json")
            self._emit(f"{name}.changed", {"seq": (snapshot or {}).get("seq")})

    def _exited(self, holder: dict[str, Runtime]) -> None:
        runtime = holder.get("runtime")
        if runtime is None:
            return
        with self._lock:
            key = runtime.data_dir.resolve()
            if self._running.get(key) is runtime:
                del self._running[key]
        self._emit("status.changed", {"seq": None})

    def _emit(self, event: str, payload: dict) -> None:
        if self._events is not None:
            try:
                self._events(event, payload)
            except Exception:
                log.exception("Kumi's %s event couldn't be sent", event)


def _read_json(path: Path) -> dict | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return value if isinstance(value, dict) else None


def _read_text(path: Path) -> str | None:
    try:
        text = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return text or None
