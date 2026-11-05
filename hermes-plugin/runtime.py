"""Kumi's runtime for Hermes: the `kumi` setup installed, `kumi serve --stdio` as a child process, MCP over its pipes.

`kumi serve` is a headless mode of Kumi's own release binary (RESEARCH.md C2). This module finds the copy setup installed
in this profile's plugin data, starts it with a scrubbed environment, checks it's the pinned runtime before anything else,
and calls its tools. A profile has one runtime: this Hermes process holds the profile's state-folder lock while its
runtime runs (CONTRACT-CHANGES CC-6), and another process's tools say where it runs. The runtime is owned by the chat
that started it, and stops when that chat ends, after 15 minutes with nothing to do, when the plugin unloads, or when
Hermes exits: its stdin closes, and it settles what's in flight and exits.

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
import queue
import stat
import subprocess
import threading
import time
from typing import Any, Callable, Iterable, Mapping

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
MAX_LINE = 16 * 1024 * 1024  # the longest message the runtime may send; a longer line is skipped
IDLE_AFTER = 15 * 60.0  # RESEARCH.md §3.1
# How long a stop waits for the runtime to settle before ending it: within the 10 s Hermes's gateway gives a chat's
# finalization, with the terminate's own wait after it.
STOP_WAIT = 5.0


class Unavailable(Exception):
    """Kumi's runtime can't be used: `code` says why in a word tools pass on, `reason` in a sentence.

    setup_needed, wrong_runtime, other_runtime, other_daw and not_running: nothing was sent. stopped (the runtime ended
    with the call in flight) and timeout (no answer came, and the call was cancelled): what it did is unknown.
    protocol: the runtime answered something that isn't C2's."""

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
    """The runtime setup installed: its version and target, the `kumi` to start, Kumi's transport home, and the ffmpeg
    the producer allowed at setup, as setup recorded it (CC-10: `{path, size, mtime_ns, ino}`), if any."""
    version: str
    target: str
    kumi: Path
    kumi_home: Path
    ffmpeg: dict | None = None


def installed(data_dir: Path, pinned: str | None, target: str | None = None) -> Install:
    """The runtime setup installed in `data_dir` (this profile's plugin data), from runtime/current.json, which setup
    writes last: only one of the pinned version, for this computer, whose `kumi` is an executable inside `data_dir`, with
    an absolute transport home if it names one. Anything else raises Unavailable("setup_needed"): setup repairs it,
    never this."""
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
    if home is not None and not (isinstance(home, str) and Path(home).is_absolute()):
        raise Unavailable("setup_needed", "Kumi's setup record names a transport home that isn't a full path: set "
                                          "Kumi up again.")
    tools = receipt.get("tools") if isinstance(receipt.get("tools"), dict) else {}
    ffmpeg = tools.get("ffmpeg") if isinstance(tools.get("ffmpeg"), dict) else None
    return Install(version, target, program, Path(home) if home else data_dir / "kumi-home", ffmpeg)


def allowed_tool(record: dict | None) -> str | None:
    """The program setup recorded (CC-10), while it's still the one the producer allowed: the absolute, canonical path
    setup recorded (none of its folders since swapped for a link, to a share or anywhere else), naming a regular,
    executable file whose size, modification time and inode are as recorded. Anything else is None: a program that
    was upgraded, swapped, moved behind a link or removed since needs setup again, and Kumi never looks for another."""
    path = record.get("path") if isinstance(record, dict) else None
    if not isinstance(path, str) or not os.path.isabs(path) or os.path.realpath(path) != path:
        return None
    try:
        found = os.stat(path, follow_symlinks=False)
    except OSError:
        return None
    same = all(isinstance(record.get(key), int) and record[key] == getattr(found, f"st_{key}")
               for key in ("size", "mtime_ns", "ino"))
    return path if same and stat.S_ISREG(found.st_mode) and os.access(path, os.X_OK) else None


def launch_environment(environ: Mapping[str, str], windows: bool = os.name == "nt") -> dict[str, str]:
    """The environment `kumi serve` starts with: the launch variables of `environ`, and no update check (rule 3)."""
    kept = {key: value for key, value in environ.items() if (key.upper() if windows else key) in LAUNCH_VARIABLES}
    kept["KUMI_NO_UPDATE_CHECK"] = "1"
    return kept


def serve_argv(install: Install, state_dir: Path, daw: str, library_roots: Iterable[str] = ()) -> list[str]:
    """`kumi serve`'s arguments (C2; W1-REGISTRY): its state folder, Kumi's transport home, the DAW, each sample folder
    the producer approved (`--library-root`, the runtime's only grant to read them), and the ffmpeg they allowed at
    setup while it's unchanged (`--tool ffmpeg=`, CC-10). Read at each start."""
    argv = [str(install.kumi), "serve", "--stdio", "--state-dir", str(state_dir), "--kumi-home", str(install.kumi_home),
            "--daw", daw]
    for root in library_roots:
        argv += ["--library-root", root]
    if ffmpeg := allowed_tool(install.ffmpeg):
        argv += ["--tool", f"ffmpeg={ffmpeg}"]
    return argv


def _thread(target: Callable[..., Any], name: str, *args: Any) -> threading.Thread:
    """A daemon thread that runs under its starter's context variables: Hermes keeps the active profile in them
    (agent.memory_provider.spawn_context_thread does the same), and a thread started without them lands on another."""
    context = contextvars.copy_context()
    return threading.Thread(target=context.run, args=(target, *args), name=name, daemon=True)


def _detached() -> dict:
    """How `kumi serve` is started apart from Hermes's terminal. A Ctrl-C meant for Hermes must not reach it and end it
    mid-write: it stops only when its stdin closes, which lets it settle first (its own session on POSIX; no console
    on Windows)."""
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NO_WINDOW}
    return {"start_new_session": True}


class StateLock:
    """One runtime per state folder (CONTRACT-CHANGES CC-6, with C6-1): an exclusive OS advisory lock on
    <state>/runtime.lock, which this Hermes process takes before it starts its runtime there.

    On POSIX the runtime inherits the locked descriptor (`keep()`), so the lock lasts until both have closed it: a
    Hermes process that dies leaves it held while its runtime settles and publishes its last snapshots. On Windows a
    lock can't be handed down, so a live runtime named in owner.json, with a fresh heartbeat, also holds the folder.
    The pid written in the file is for the message another process gets, never the lock itself."""

    def __init__(self, path: Path):
        self.path = path
        self._file = None

    def acquire(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        file = open(self.path, "a+b")
        try:
            _lock(file)
        except OSError:
            holder = _holder(file)
            file.close()
            raise _other_runtime(holder) from None
        owner = _live_owner(self.path.parent) if os.name == "nt" else None
        if owner is not None:
            _unlock(file)
            file.close()
            raise _other_runtime(owner)
        file.seek(0)
        file.truncate()
        file.write(json.dumps({"pid": os.getpid(), "started_at": int(time.time() * 1000)}).encode("utf-8"))
        file.flush()
        self._file = file

    def keep(self) -> tuple[int, ...]:
        """The descriptors the runtime inherits to hold the lock with this process (none on Windows)."""
        return (self._file.fileno(),) if self._file is not None and os.name != "nt" else ()

    def release(self) -> None:
        """This process's hold ends (its runtime has exited). On POSIX only closing it, never an unlock, which would
        free the lock the runtime may still share."""
        file, self._file = self._file, None
        if file is not None:
            try:
                if os.name == "nt":
                    _unlock(file)
            finally:
                file.close()


def _other_runtime(pid: int | None) -> Unavailable:
    where = f"another Hermes process (pid {pid})" if pid else "another Hermes process"
    return Unavailable("other_runtime", f"Kumi's runtime for this profile runs in {where}: use that chat, or end it.")


if os.name == "nt":
    import ctypes
    import msvcrt

    # Windows's locks are mandatory: the locked byte lies far past the pid written at the start, which another process
    # then can still read for its message.
    _LOCKED_BYTE = 1 << 30

    def _lock(file) -> None:
        file.seek(_LOCKED_BYTE)
        msvcrt.locking(file.fileno(), msvcrt.LK_NBLCK, 1)

    def _unlock(file) -> None:
        file.seek(_LOCKED_BYTE)
        msvcrt.locking(file.fileno(), msvcrt.LK_UNLCK, 1)

    def _alive(pid: int) -> bool:
        # (os.kill(pid, 0) would end the process on Windows.)
        kernel = ctypes.windll.kernel32
        handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not handle:
            return False
        try:
            code = ctypes.c_ulong()
            return bool(kernel.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259  # STILL_ACTIVE
        finally:
            kernel.CloseHandle(handle)
else:
    import fcntl

    def _lock(file) -> None:
        fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

    def _alive(pid: int) -> bool:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True


def _holder(file) -> int | None:
    try:
        file.seek(0)
        pid = json.loads(file.read(4096) or b"{}").get("pid")
    except (OSError, ValueError, AttributeError):
        return None
    return pid if isinstance(pid, int) else None


def _live_owner(state_dir: Path, now_ms: int | None = None) -> int | None:
    """The pid of a runtime that owner.json (C3) says holds this state folder, when it's alive and its heartbeat is
    under 10 s old."""
    owner = _read_json(state_dir / "owner.json") or {}
    pid = (owner.get("owner") or {}).get("pid") if isinstance(owner.get("owner"), dict) else None
    heartbeat = owner.get("heartbeat")
    now_ms = int(time.time() * 1000) if now_ms is None else now_ms
    if not isinstance(pid, int) or not isinstance(heartbeat, (int, float)) or now_ms - heartbeat > 10_000:
        return None
    return pid if _alive(pid) else None


@dataclass
class _Pending:
    done: threading.Event = field(default_factory=threading.Event)
    result: Any = None
    error: dict | None = None
    gone: str | None = None


class Client:
    """MCP over a child's stdin and stdout (the stdio transport: one JSON-RPC message a line). Requests are answered by
    id; notifications go to `on_notification` on the reader thread. A writer thread sends, so a runtime that stops
    reading never blocks a caller. When the child exits, every waiting request fails with Unavailable("stopped"), and
    nothing is sent again."""

    def __init__(self, argv: list[str], env: dict[str, str], *, cwd: str | None = None,
                 on_notification: Callable[[str, dict], None] | None = None,
                 on_exit: Callable[[str], None] | None = None, exit_wait: float = 5.0, keep_fds: tuple[int, ...] = ()):
        try:
            self._process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                             stderr=subprocess.PIPE, env=env, cwd=cwd, close_fds=True,
                                             pass_fds=keep_fds, **_detached())
        except OSError as failure:
            raise Unavailable("setup_needed", f"Kumi's runtime couldn't start ({failure.strerror or failure}): set Kumi "
                                              "up again.") from None
        self._ids = itertools.count(1)
        self._pending: dict[int, _Pending] = {}
        self._lock = threading.Lock()
        self._outbox: queue.Queue[bytes | None] = queue.Queue()
        self._gone: str | None = None
        self._on_notification = on_notification
        self._on_exit = on_exit
        self._exit_wait = exit_wait
        _thread(self._read, "kumi-serve-stdout").start()
        _thread(self._write, "kumi-serve-stdin").start()
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
                raise Unavailable("not_running", f"{self._gone} Nothing was sent.")
            ident = next(self._ids)
            self._pending[ident] = pending
            self._outbox.put(_line({"jsonrpc": "2.0", "id": ident, "method": method, "params": params or {}}))
        try:
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
        with self._lock:
            if self._gone is None:
                self._outbox.put(_line({"jsonrpc": "2.0", "method": method, "params": params or {}}))

    def close(self, timeout: float = STOP_WAIT) -> None:
        """End the runtime's stdin once what's queued is written: it settles what's in flight and exits. One still
        running after `timeout` seconds is terminated, then killed."""
        self._outbox.put(None)
        try:
            self._process.wait(timeout)
        except subprocess.TimeoutExpired:
            log.warning("kumi serve (pid %s) didn't exit %g s after its stdin closed: stopping it", self.pid, timeout)
            _end(self._process)

    def _write(self) -> None:
        try:
            while (line := self._outbox.get()) is not None:
                self._process.stdin.write(line)
                self._process.stdin.flush()
        except (OSError, ValueError):
            pass
        finally:
            try:
                self._process.stdin.close()
            except (OSError, ValueError):
                pass

    def _read(self) -> None:
        stream = self._process.stdout
        try:
            while raw := stream.readline(MAX_LINE + 1):
                if len(raw) > MAX_LINE and not raw.endswith(b"\n"):
                    log.warning("kumi serve wrote a line over %d bytes: skipped", MAX_LINE)
                    while raw and not raw.endswith(b"\n"):
                        raw = stream.readline(MAX_LINE + 1)
                    continue
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
            stream.close()
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
                reply = {"jsonrpc": "2.0", "id": message["id"], "result": {}}
            else:
                reply = {"jsonrpc": "2.0", "id": message["id"],
                         "error": {"code": -32601, "message": f"{method} isn't offered by Kumi's Hermes plugin"}}
            self._outbox.put(_line(reply))
        elif self._on_notification is not None:
            try:
                self._on_notification(method, message.get("params") or {})
            except Exception:
                log.exception("Kumi's handling of the runtime's %s failed", method)

    def _exited(self) -> None:
        """The runtime's output ended: it's exiting. Every waiting call fails, nothing more is sent, and a runtime that
        closed its output but goes on running is ended rather than left behind."""
        code = self._process.poll()
        reason = "Kumi's runtime stopped" + (f" (exit {code})." if code is not None else ".")
        with self._lock:
            self._gone = reason
            waiting = list(self._pending.values())
            self._outbox.put(None)
        for pending in waiting:
            if not pending.done.is_set():  # an answer read before the end stands
                pending.gone = reason
                pending.done.set()
        try:
            self._process.wait(self._exit_wait)
        except subprocess.TimeoutExpired:
            log.warning("kumi serve (pid %s) closed its output and is still running: stopping it", self.pid)
            _end(self._process)
        if self._on_exit is not None:
            try:
                self._on_exit(reason)
            except Exception:
                log.exception("Kumi's handling of its runtime's exit failed")

    def _log_stderr(self) -> None:
        stream = self._process.stderr
        try:
            while raw := stream.readline(64 * 1024):
                log.info("kumi serve: %s", raw.decode("utf-8", "replace").rstrip()[:2000])
        except (OSError, ValueError):
            pass
        finally:
            stream.close()


def _line(message: dict) -> bytes:
    return json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"


def _end(process: subprocess.Popen) -> None:
    process.terminate()
    try:
        process.wait(2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


@dataclass
class Runtime:
    """A started `kumi serve`: its client, what it runs, the chat that owns it, and whether a call is in flight."""
    client: Client
    install: Install
    data_dir: Path
    daw: str
    owner_session: str | None
    state_lock: StateLock
    info: dict = field(default_factory=dict)
    last_used: float = field(default_factory=time.monotonic)
    in_flight: int = 0
    closing: bool = False
    retired: bool = False
    gone: threading.Event = field(default_factory=threading.Event)
    _lock: threading.Lock = field(default_factory=threading.Lock)

    @property
    def state_dir(self) -> Path:
        return self.data_dir / "state"

    def call(self, tool: str, args: dict, host: dict, *, timeout: float) -> dict:
        """Call one of the runtime's tools (C2: C1's names without `kumi_`), with Hermes's call identity in `_host`.
        Its answer's structured content, or Refused with the runtime's error."""
        with self._lock:
            if self.closing:
                raise Unavailable("not_running", "Kumi's runtime was stopping; kumi_connect starts it again. Nothing "
                                                 "was sent.")
            self.in_flight += 1
        try:
            result = self.client.request("tools/call", {"name": tool, "arguments": {**args, "_host": host}},
                                         timeout=timeout)
        finally:
            with self._lock:
                self.in_flight -= 1
                self.last_used = time.monotonic()
        content = result.get("structuredContent") if isinstance(result, dict) else None
        if not isinstance(content, dict):
            raise Unavailable("protocol", f"Kumi's {tool} answered without structured content.")
        if result.get("isError"):
            error = content.get("error") if isinstance(content.get("error"), dict) else {}
            raise Refused(str(error.get("code") or "failed"), str(error.get("reason") or f"Kumi's {tool} failed."),
                          error.get("owner") if isinstance(error.get("owner"), dict) else None)
        return content

    def close_if_idle(self, idle_after: float, now: float) -> bool:
        """Mark this runtime closing when it has had nothing to do for `idle_after` seconds: true when it's to stop."""
        with self._lock:
            if self.closing or self.in_flight or now - self.last_used < idle_after:
                return False
            self.closing = True
            return True


Spawn = Callable[..., Client]


class Runtimes:
    """This process's runtimes: one per profile (its plugin-data folder), each owned by the chat that started it.
    `events(name, payload)` is told when a snapshot changes (C5: `status.changed`, `focus.changed`, …)."""

    def __init__(self, *, events: Callable[[str, dict], None] | None = None, spawn: Spawn = Client,
                 environ: Callable[[], Mapping[str, str]] = lambda: os.environ, start_timeout: float = 30.0,
                 idle_after: float = IDLE_AFTER):
        self._events = events
        self._spawn = spawn
        self._environ = environ
        self._start_timeout = start_timeout
        self._idle_after = idle_after
        self._lock = threading.Lock()  # the registry only: never held while a runtime starts or stops
        self._running: dict[Path, Runtime] = {}
        self._starting: dict[Path, threading.Lock] = {}
        # Starts under way (profile → the chat starting it), and those a stop overtook: they end as they finish.
        self._pending: dict[Path, str | None] = {}
        self._cancelled: set[Path] = set()
        self._watching = False

    def get(self, data_dir: Path) -> Runtime | None:
        with self._lock:
            runtime = self._running.get(data_dir.resolve())
        return runtime if runtime is not None and runtime.client.running and not runtime.closing else None

    def start(self, data_dir: Path, install: Install, daw: str, session_id: str | None, *,
              library_roots: Iterable[str] = ()) -> Runtime:
        """The profile's runtime, started for `session_id` when none runs: this process takes the profile's state
        lock (CC-6), then the runtime is initialized, checked to be the pinned runtime (its name, version and protocol,
        before anything else is asked of it) and subscribed to its snapshots. A runtime that isn't the pinned one is
        stopped and Unavailable("wrong_runtime") raised. Another profile's start never waits for this one."""
        key = data_dir.resolve()
        with self._lock:
            starting = self._starting.setdefault(key, threading.Lock())
        with starting:
            running = self.get(data_dir)
            if running is not None:
                if running.daw != daw:
                    raise Unavailable("other_daw", f"Kumi's runtime in this profile works in {running.daw}, not {daw}.")
                return running
            state = data_dir / "state"
            lock = StateLock(state / "runtime.lock")
            lock.acquire()
            with self._lock:
                self._pending[key] = session_id
                self._cancelled.discard(key)
            client = None
            try:
                install.kumi_home.mkdir(parents=True, exist_ok=True)
                holder: dict[str, Runtime] = {}
                client = self._spawn(serve_argv(install, state, daw, library_roots), launch_environment(self._environ()),
                                     cwd=str(data_dir), keep_fds=lock.keep(),
                                     on_notification=lambda method, params: self._notified(holder, method, params),
                                     on_exit=lambda reason: self._exited(holder))
                runtime = Runtime(client, install, data_dir, daw, session_id, lock)
                runtime.info = self._handshake(runtime)
                holder["runtime"] = runtime
                with self._lock:  # the mark is checked and the runtime published as one step, so no stop falls between
                    cancelled = key in self._cancelled
                    self._pending.pop(key, None)
                    self._cancelled.discard(key)
                    if not cancelled:
                        self._running[key] = runtime
                        watch, self._watching = not self._watching, True
                if cancelled:  # its chat ended, or the plugin unloaded, while it started
                    raise Unavailable("not_running", "Kumi's runtime was stopped as it started (its chat ended, or "
                                                     "the plugin unloaded). Nothing was sent.")
            except BaseException:
                with self._lock:
                    self._pending.pop(key, None)
                    self._cancelled.discard(key)
                if client is not None:
                    client.close(timeout=STOP_WAIT)
                lock.release()
                raise
        if watch:
            _thread(self._watch_idle, "kumi-runtime-idle").start()
        self._emit("status.changed", {"seq": None})
        return runtime

    def stop(self, data_dir: Path, *, wait: float = STOP_WAIT) -> None:
        key = data_dir.resolve()
        with self._lock:
            runtime = self._running.get(key)
            if key in self._pending:
                self._cancelled.add(key)
        if runtime is not None:
            runtime.closing = True
            runtime.client.close(timeout=wait)
            self._retire(runtime)

    def stop_session(self, session_id: str | None) -> None:
        """Stop the runtimes `session_id` owns (its chat ended), a start of its own under way included."""
        if not session_id:
            return
        with self._lock:
            owned = {key for key, runtime in self._running.items() if runtime.owner_session == session_id}
            owned |= {key for key, starter in self._pending.items() if starter == session_id}
        for key in owned:
            self.stop(key)

    def stop_all(self) -> None:
        with self._lock:
            keys = set(self._running) | set(self._pending)
        for key in keys:
            self.stop(key)

    def _watch_idle(self) -> None:
        """Stop each runtime that has had nothing to do for `idle_after` seconds, never one with a call in flight; a
        runtime no chat owns (cron, a call without a session) stops this way too."""
        while True:
            time.sleep(min(60.0, max(self._idle_after / 4, 0.01)))
            with self._lock:
                if not self._running:
                    self._watching = False
                    return
                runtimes = list(self._running.values())
            for runtime in runtimes:
                if runtime.close_if_idle(self._idle_after, time.monotonic()):
                    log.info("Kumi's runtime for %s was idle for %g s: stopping it", runtime.data_dir, self._idle_after)
                    runtime.client.close(timeout=STOP_WAIT)
                    self._retire(runtime)

    def _handshake(self, runtime: Runtime) -> dict:
        client, expected = runtime.client, runtime.install.version
        deadline = time.monotonic() + self._start_timeout

        def left() -> float:
            return max(deadline - time.monotonic(), 0.01)

        result = client.request("initialize", {"protocolVersion": MCP_PROTOCOL, "capabilities": {},
                                               "clientInfo": CLIENT_INFO}, timeout=left())
        info = result.get("serverInfo") if isinstance(result, dict) else None
        info = info if isinstance(info, dict) else {}
        if info.get("name") != SERVER_NAME or info.get("version") != expected:
            raise Unavailable("wrong_runtime", f"The runtime in Kumi's install is {info.get('name')} "
                                               f"{info.get('version')}, not {SERVER_NAME} {expected}: set Kumi up again.")
        if result.get("protocolVersion") != MCP_PROTOCOL:
            raise Unavailable("wrong_runtime", f"Kumi's runtime speaks MCP {result.get('protocolVersion')}, not "
                                               f"{MCP_PROTOCOL}: set Kumi up again.")
        client.notify("notifications/initialized")
        status = runtime.call("status", {}, {"profile_id": "", "session_id": runtime.owner_session, "task_id": None,
                                             "tool_call_id": None, "request_id": "handshake"}, timeout=left())
        protocol = status.get("runtime", {}).get("protocol") if isinstance(status.get("runtime"), dict) else None
        if protocol != RUNTIME_PROTOCOL:
            raise Unavailable("wrong_runtime", f"Kumi's runtime speaks {protocol}, not {RUNTIME_PROTOCOL}: set Kumi up "
                                               "again.")
        if (result.get("capabilities") or {}).get("resources", {}).get("subscribe"):
            self._subscribe(client, left)
        return info

    def _subscribe(self, client: Client, left: Callable[[], float]) -> None:
        """Subscribe to each of C2's snapshots the runtime lists. One it refuses goes without events, and the UIs' poll
        still sees it (C5)."""
        try:
            listed = client.request("resources/list", {}, timeout=left())
        except Unavailable as failure:
            if failure.code != "protocol":
                raise
            log.warning("Kumi's runtime didn't list its resources: no snapshot events (%s)", failure.reason)
            return
        uris = {resource.get("uri") for resource in (listed or {}).get("resources") or [] if isinstance(resource, dict)}
        for uri in RESOURCES:
            if uri in uris:
                try:
                    client.request("resources/subscribe", {"uri": uri}, timeout=left())
                except Unavailable as failure:
                    if failure.code != "protocol":
                        raise
                    log.warning("Kumi's runtime refused a subscription to %s: %s", uri, failure.reason)

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
        if runtime is not None:
            self._retire(runtime)

    def _retire(self, runtime: Runtime) -> None:
        """A runtime that has exited leaves the registry and frees the profile's state lock, once, telling the UIs. The
        stop and the runtime's own exit both come here: whichever is second returns once the first is done."""
        with self._lock:
            first, runtime.retired = not runtime.retired, True
            key = runtime.data_dir.resolve()
            if self._running.get(key) is runtime:
                del self._running[key]
        if not first:
            runtime.gone.wait(STOP_WAIT)
            return
        try:
            runtime.state_lock.release()
            self._emit("status.changed", {"seq": None})
        finally:
            runtime.gone.set()

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
