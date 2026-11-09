"""Kumi's Hermes tools (RESEARCH.md C1): each handler takes the model's arguments and Hermes's call context, and returns
JSON. A handler never raises into Hermes: what goes wrong comes back as `{"error": {"code", "reason"}}`.
"""
from __future__ import annotations

import json
import logging
import math
import os
from pathlib import Path
import re
import threading
import time
from typing import Any, Callable
import uuid

from . import calls, devices, generate, listening, palettes, runtime, schemas, scripts, setup, snapshots

log = logging.getLogger(__name__)

PLUGIN = "kumi"
TOOLSET = "kumi"
STATUS_TIMEOUT = 15.0
CONNECT_TIMEOUT = 60.0
READ_TIMEOUT = 30.0
WRITE_TIMEOUT = 120.0
# Installing Kumi's part in a DAW unpacks and copies its bridge: it gets longer than a change.
SETUP_TIMEOUT = 300.0
UNSURE_HINT = ("What it did is unknown. Don't send it again: check kumi_history and the project (kumi_observe) "
               "first.")
# How a write that was sent can end with its outcome unknown: no answer in time, the producer's Stop (both cancelled),
# the runtime gone, or an answer that couldn't be read.
UNSURE_CODES = ("timeout", "interrupted", "stopped", "protocol")


class Host:
    """What the tools need from Hermes, read on every call: the active profile can change between calls, and with it
    the plugin-data folder and the settings."""

    def __init__(self, ctx: Any, runtimes: runtime.Runtimes, *, data_dir: Callable[[], Path] | None = None,
                 pinned: Callable[[], str | None] = setup.pinned_version,
                 hook_timeout: Callable[[], float | None] | None = None, plugin_dir: Path | None = None,
                 hermes_config: Callable[[], dict] | None = None, session_key: Callable[[], str] | None = None,
                 media_roots: Callable[[], list[str]] | None = None, ledger: Callable[..., Any] | None = None,
                 mirelo_key: Callable[[], str | None] | None = None):
        self._ctx = ctx
        self.runtimes = runtimes
        self.calls = calls.CallIds()
        self.turns = calls.Turns()  # the producer's own turns, so a turn Hermes forks in the background is told apart
        self._data_dir = data_dir
        self._pinned = pinned
        self._hook_timeout = hook_timeout
        self.plugin_dir = plugin_dir  # where setup reads kumi_release.json (None: this plugin's own folder)
        self._hermes_config = hermes_config
        self._ledger = ledger
        self._mirelo_key = mirelo_key
        self._session_key = session_key
        self._media_roots = media_roots
        self._stops: dict[str, float] = {}  # a session's key → when the producer last stopped its turn
        self._stops_lock = threading.Lock()
        # A chat's session id → the HISTORY ids of each change its kumi_change made, oldest first, in this Hermes
        # process: what kumi_undo with no id undoes, whole (CC-3 amendment A2), never another chat's. With the rows
        # this process saw undone, a second "undo that" reaches the change before (rgzm, #1088).
        self._changes: dict[str, list[list[str] | str]] = {}  # each chat's writes: a change's HISTORY ids, or a file
        self._undone: set[str] = set()
        self._changes_lock = threading.Lock()
        self._trusts: dict[Path, scripts.Trust] = {}  # each profile's record of which chats read outside content
        self._trusts_lock = threading.Lock()
        self._listen_lock = threading.Lock()
        self._listen_registration: dict | None = None
        self._listen_handles: list[Any] = []
        self._listen_features = frozenset()
        self._unloaded = False

    def register_listen(self, handler: Callable, check_fn: Callable | None, emoji: str) -> None:
        """Register once without I/O; keep these exact SDK callables on every later schema refresh."""
        with self._listen_lock:
            if self._unloaded:
                return
            self.runtimes.on_change = self.refresh_listen_schema
            self._listen_registration = {"name": "kumi_listen", "toolset": TOOLSET, "handler": handler,
                                         "check_fn": check_fn, "emoji": emoji}
            self._keep_listen_handle(self._ctx.register_tool(**self._listen_registration, schema=schemas.listen_schema()))

    def refresh_listen_schema(self, data_dir: Path) -> None:
        # Compute and register under one lock: a late start/retire callback must not restore stale feature bytes.
        with self._listen_lock:
            if self._unloaded or self._listen_registration is None:
                return
            live = self.runtimes.get(data_dir)
            features = live.listen_features if live is not None else frozenset()
            if features == self._listen_features:
                return
            try:
                self._keep_listen_handle(self._ctx.register_tool(**self._listen_registration,
                                                                 schema=schemas.listen_schema(features)))
            except Exception:
                log.exception("Kumi couldn't refresh its listening schema; per-call feature checks still apply")
                return
            self._listen_features = features

    def _keep_listen_handle(self, handle: Any) -> None:
        if handle is not None:
            self._listen_handles.append(handle)

    def unload(self) -> None:
        with self._listen_lock:
            self._unloaded = True
            handles, self._listen_handles = self._listen_handles, []
        # Hermes snapshots its ledger before callbacks. A refresh already inside register_tool at that instant
        # may not be in that snapshot; dispose our supported handles as well (SDK disposal is idempotent).
        for handle in reversed(handles):
            try:
                handle.dispose()
            except Exception:
                log.exception("Kumi couldn't dispose its listening registration")
        self.runtimes.stop_all()

    def remember_change(self, kwargs: dict, answer: Any) -> None:
        """Keep a chat's write that made HISTORY rows (a change applied or partial, a script that ran or failed, a
        make of takes) as its newest change, so "undo that" means it, never one before it."""
        if not isinstance(answer, dict) or answer.get("state") not in ("applied", "partial", "ran", "failed", "done"):
            return
        session, ids = kwargs.get("session_id"), answer.get("history_ids")
        if (isinstance(session, str) and session and isinstance(ids, list) and ids
                and all(isinstance(each, str) and each for each in ids)):
            with self._changes_lock:
                self._changes[session] = [*self._changes.get(session, [])[1 - CHANGES_KEPT:], list(ids)]

    def remember_undo(self, asked: dict, answer: Any) -> None:
        """Note the rows an undo took back: the one row it named, or each row of a whole change its answer says."""
        if not isinstance(answer, dict):
            return
        rows = answer.get("rows") if isinstance(answer.get("rows"), list) else []
        undone = {row.get("history_id") for row in rows if isinstance(row, dict) and row.get("state") == "undone"}
        if not rows and answer.get("state") == "undone":
            undone = {asked.get("history_id")}
        with self._changes_lock:
            self._undone |= {each for each in undone if isinstance(each, str)}

    def remember_library_write(self, kwargs: dict, file: Any) -> None:
        """Keep a chat's write of a new file outside the Set (CC-22's device build, a preset, a wavetable) as its
        newest step. Kumi never undoes one: the file stays, as its approval said. So "undo that" right after it undoes
        nothing, never the change before it (CC-26's 26-w5)."""
        session = kwargs.get("session_id")
        if isinstance(session, str) and session and isinstance(file, str) and file:
            with self._changes_lock:
                self._changes[session] = [*self._changes.get(session, [])[1 - CHANGES_KEPT:], file]

    def newest_write(self, kwargs: dict) -> tuple[str, str] | None:
        """This chat's newest write Kumi remembers: ("change", the last HISTORY id of its newest change not yet
        undone), ("file", a User Library file written after it), or None when it has neither here."""
        session = kwargs.get("session_id")
        with self._changes_lock:
            writes = self._changes.get(session, []) if isinstance(session, str) else []
            for write in reversed(writes):
                if isinstance(write, str):
                    return "file", write
                if not set(write) <= self._undone:
                    return "change", write[-1]
        return None

    def on_loop_stopped(self, session_key: Any = "", **kwargs: Any) -> None:
        """Hermes's public `agent_loop_stopped` hook: the producer stopped a turn (the TUI's or Desktop's Stop, a
        messaging gateway's /stop), named by its session key. A Kumi call of that turn still waiting on the runtime is
        cancelled (`stop_watch`; research-1's review, item 7). Hermes's classic CLI doesn't fire it."""
        if isinstance(session_key, str) and session_key:
            with self._stops_lock:
                self._stops[session_key] = time.monotonic()

    def stop_watch(self) -> Callable[[], bool] | None:
        """For a call starting now, whether its turn has been stopped since (`on_loop_stopped`), asked while it waits on
        the runtime; None where Hermes doesn't name the calling session's key (`HERMES_SESSION_KEY`, its public
        session variable), and then nothing is cancelled."""
        key = self._session_key() if self._session_key is not None else _session_values("HERMES_SESSION_KEY")[0]
        if not key:
            return None
        started = time.monotonic()

        def stopped() -> bool:
            with self._stops_lock:
                return self._stops.get(key, float("-inf")) >= started

        return stopped

    def hermes_config(self) -> dict:
        """Hermes's own settings (its config.yaml, as Hermes reads it), for the few Kumi follows; read on each use."""
        if self._hermes_config is not None:
            return self._hermes_config()
        try:
            from hermes_cli.config import load_config_readonly  # Hermes's
            config = load_config_readonly()
        except Exception:
            return {}
        return config if isinstance(config, dict) else {}

    def mirelo_key(self) -> str | None:
        """The producer's Mirelo API key, read on each call: plugin.yaml's `mirelo_api_key` secret, which Hermes keeps in
        the profile's .env as MIRELO_API_KEY and gives through its public, profile-scoped `get_env_value`. Kumi never
        opens .env itself, and the key goes only into one request's `_mirelo`, after the producer's approval (13-R3)."""
        if self._mirelo_key is not None:
            return self._mirelo_key()
        try:
            from hermes_cli.config import get_env_value  # Hermes's
            value = get_env_value(generate.KEY_ENV)
        except Exception:
            return None
        return value if isinstance(value, str) and value.strip() else None

    def generation(self) -> dict:
        """The producer's Mirelo settings (CC-13), read on each call: acknowledged, each kind switched on, and the
        lifetime credit cap (0, the default, refuses every paid generation). None of it comes from the model."""
        cap = self._ctx.get_config("mirelo_credit_cap", 0)
        return {"acknowledged": self._ctx.get_config("mirelo_acknowledged", False) is True,
                "enabled": {kind: self._ctx.get_config(setting, False) is True
                            for kind, setting in generate.SETTINGS.items()},
                "cap": cap if isinstance(cap, int) and not isinstance(cap, bool) and cap >= 0 else 0}

    def mirelo_status(self) -> dict:
        """Mirelo's readiness as `hermes kumi doctor` reports it (#1058): whether the producer's key is held (never the
        key), the onboarding acknowledged, a kind switched on, and a credit cap above 0. Settings only: the ledger
        isn't opened, so nothing is written (rgzm)."""
        settings = self.generation()
        return {"key": self.mirelo_key() is not None, "acknowledged": settings["acknowledged"],
                "task": any(settings["enabled"].values()), "cap": settings["cap"] > 0}

    def ledger(self) -> Any:
        """This profile's credit ledger (W3's `ledger.Ledger`, under its own file lock), with the cap as the producer
        set it now, and their acknowledgement and each kind's switch written into it (`configure`): the runtime reads
        that policy and a create's reservation from the ledger, never from `_mirelo`'s copy alone (agent-m). Raises
        generate.LedgerUnavailable when it can't be had."""
        settings = self.generation()
        try:
            opener = self._ledger
            if opener is None:
                from .ledger import Ledger as opener  # W3's
            ledger = opener(Path(self.data_dir()), credit_cap=settings["cap"], lock_timeout=generate.LEDGER_WAIT)
            ledger.configure(acknowledged=settings["acknowledged"], enabled=settings["enabled"])
            return ledger
        except Exception as failure:
            log.warning("Kumi couldn't open its credit ledger: %s", type(failure).__name__)
            raise generate.LedgerUnavailable("ledger_unavailable",
                                             "Kumi can't read its credit ledger, so nothing is paid.") \
                from None

    def session_surface(self) -> str:
        """The calling session's client (`cli`, `tui`, `desktop`, `acp`) or messaging platform (`telegram`, …), as
        Hermes names it (`ctx.session_surface()`): "" outside a session or on a Hermes without it, and "unknown" when
        asking fails, which `policy.shows_whole` takes as a place that shows nothing whole."""
        ask = getattr(self._ctx, "session_surface", None)
        if ask is None:
            return ""
        try:
            surface = ask()
        except Exception:
            log.warning("Kumi couldn't ask Hermes where this call comes from: taken as unknown", exc_info=True)
            return "unknown"
        return surface if isinstance(surface, str) else "unknown"

    def hook_timeout(self) -> float | None:
        """Hermes's `plugins.hook_callback_timeout` (default 30 s; 0 or less: none), read on each call."""
        if self._hook_timeout is not None:
            return self._hook_timeout()
        from hermes_cli.config import load_config_readonly  # Hermes's, documented
        plugins = (load_config_readonly() or {}).get("plugins")
        value = plugins.get("hook_callback_timeout") if isinstance(plugins, dict) else None
        try:
            return 30.0 if value is None else float(value)
        except (TypeError, ValueError):
            return 30.0

    def data_dir(self) -> Path:
        """`data_path`, created (empty) on the first call, as Hermes's `plugin_data_dir` does; the only write a read
        tool makes."""
        path = self.data_path()
        if path is None:
            raise RuntimeError("Hermes's home can't be read, so Kumi has no plugin data folder")
        path.mkdir(parents=True, exist_ok=True)
        return path

    def data_path(self) -> Path | None:
        """Where Kumi's plugin data is, without creating it: `setup.default_data_dir` (Hermes's documented
        `<hermes home>/plugin-data/kumi`, `plugin_data_dir`'s folder), or the tests' own. What only reads
        (`hermes kumi doctor`) uses it. None where Hermes's home can't be read."""
        if self._data_dir is not None:
            return self._data_dir()
        try:
            return setup.default_data_dir()
        except Exception:  # Hermes's home isn't readable (hermes_constants missing or failing)
            return None

    def daw(self) -> str:
        daw = self._ctx.get_config("daw", "ableton")
        return daw if daw in schemas.DAWS else "ableton"

    def inject_focus(self) -> bool:
        return self._ctx.get_config("inject_focus", True) is not False

    def library_folders(self) -> list[str]:
        """The sample folders in the plugin's settings, as the guard keeps them (`snapshots.library_folders`)."""
        return snapshots.library_folders(self._ctx.get_config("library_folders", []), hermes_home=self.hermes_home())

    def hermes_home(self) -> str | None:
        return snapshots.hermes_home()

    def listen_interpretation(self) -> bool:
        return self._ctx.get_config("listen_interpretation", False) is True

    def scripts_in_messaging(self) -> bool:
        """CC-19's 19d: the producer let messaging chats (Discord, Slack, Telegram…) run scripts, each one asked."""
        return self._ctx.get_config("scripts_in_messaging", False) is True

    def ask_before_changes(self) -> bool:
        """The producer's "Ask before each change": on, Hermes asks before each change Kumi makes in the DAW
        (`policy.UNASKED_BY_DEFAULT`, scripts and devices included); off, the default, Kumi makes them without asking.
        Setup, sample folders, and Mirelo's uploads and paid jobs are asked either way, as is a script or a device in a
        chat that read outside content."""
        return self._ctx.get_config("ask_before_changes", False) is True

    def trust(self) -> scripts.Trust:
        """This profile's record of which chats read outside content (CC-19 r1), in its plugin data."""
        path = self.data_dir() / scripts.TRUST_FILE
        with self._trusts_lock:
            return self._trusts.setdefault(path, scripts.Trust(path))

    def llm(self) -> Any:
        """Hermes's model facade for this plugin (CC-1), read per call: never while registering."""
        return self._ctx.llm

    def interpretation_timeout(self) -> float:
        """The listening task's budget for one attempt (CC-1, rgzm's note): its whole time
        (`auxiliary.kumi_listen.timeout`, else the task's default) split over Hermes's attempts, 1 plus
        `auxiliary.transient_retries` (default 2, at most 6)."""
        total, retries = float(listening.AUX_TASK["defaults"]["timeout"]), 2
        auxiliary = self.hermes_config().get("auxiliary")
        task = auxiliary.get(listening.AUX_TASK_KEY) if isinstance(auxiliary, dict) else None
        configured = task.get("timeout") if isinstance(task, dict) else None
        if isinstance(configured, (int, float)) and not isinstance(configured, bool) and 0 < configured < math.inf:
            total = float(configured)
        configured = auxiliary.get("transient_retries") if isinstance(auxiliary, dict) else None
        if isinstance(configured, int) and not isinstance(configured, bool):
            retries = max(0, min(configured, 6))
        return total / (1 + retries)

    def media_roots(self) -> list[str]:
        """Folders of finished media the runtime may import or measure from (`--media-root`): this profile's
        `<HERMES_HOME>/cache/generated/audio`, where other plugins' results land (ElevenLabs Music and Sound Effects).
        Passed as is, even before it exists: the runtime keeps an absolute root and checks a file against it when it
        resolves the file (CC-11). Kumi never makes the folder; it writes only in its plugin data."""
        if self._media_roots is not None:
            return self._media_roots()
        try:
            from hermes_constants import get_hermes_home  # Hermes's
            return [str(Path(get_hermes_home()) / "cache" / "generated" / "audio")]
        except Exception:
            log.exception("Kumi couldn't find Hermes's generated audio folder")
            return []

    def set_library_folders(self, folders: list[str]) -> None:
        self._ctx.set_config("library_folders", folders)

    def set_daw(self, daw: str) -> None:
        """Make `daw` Kumi's DAW in this profile (the plugin's `daw` setting), after the producer approved a setup
        plan that says so."""
        self._ctx.set_config("daw", daw)

    def install(self, data_dir: Path) -> runtime.Install:
        return runtime.installed(data_dir, self._pinned())

    def live(self) -> runtime.Runtime:
        """This profile's running runtime, or Unavailable saying what to do first (`busy` while one is still
        stopping)."""
        data_dir = self.data_dir()
        self.install(data_dir)
        return self.runtimes.require(data_dir)

    def profile(self) -> str:
        """The Hermes profile this call runs in (its plugin data is that profile's)."""
        return str(getattr(self._ctx, "profile_name", "") or "")

    def unattended(self) -> str | None:
        """Who can answer for this call: "scheduled" in a Hermes cron job (no chat, and nobody there), "unattended"
        where nobody can answer an approval (a webhook, Hermes's API server, a one-shot `hermes -q` run in this
        process), else None (a chat). Hermes's `ctx` methods are answered in the calling context, under
        `plugins.isolation: host` too. When Hermes can't say, nobody is assumed there.

        A cron job is told by `ctx.current_cron_execution()`, bound for each fire. A webhook or an API call is told by
        `ctx.session_surface()`. Hermes's session variables (`gateway.session_context.get_session_env`, which falls back
        to the process environment, or the environment itself when that isn't importable) are read too.

        A stated gap: in Hermes v0.22.7 a `-q` or `-z` turn's tools run in the gateway (PLG-7), where neither is told
        apart from a chat: `-q`'s mark stays in its own process, and `-z`'s (which Hermes treats as --yolo) is
        private."""
        try:
            if self._ctx.current_cron_execution() is not None:
                return "scheduled"
        except Exception:
            log.warning("Kumi couldn't ask Hermes whether this call is a scheduled job: taken as one", exc_info=True)
            return "scheduled"
        try:
            surface = str(self._ctx.session_surface() or "")
        except Exception:
            log.warning("Kumi couldn't ask Hermes which surface this call comes from: taken as unattended",
                        exc_info=True)
            return "unattended"
        cron, single, platform = _session_values("HERMES_CRON_SESSION", "HERMES_SINGLE_QUERY_SESSION",
                                                 "HERMES_SESSION_PLATFORM")
        if _truthy(cron):
            return "scheduled"
        if _truthy(single) or surface in UNATTENDED_SURFACES or platform in UNATTENDED_SURFACES:
            return "unattended"
        return None

    def identity(self, kwargs: dict) -> dict:
        """Hermes's identity for one call, which the runtime records and dedups on (C2 `_host`, CC-5), and keys a
        preview's pins on, so a hook's preview and its dispatch name the same call. Hermes gives the approval hook the
        model's tool-call id (`tool_call_id`), and a tool handler only its session and task, so a handler takes the id
        from the hook's record of this call (`kumi_call`). Null without either (never made up)."""
        call = kwargs.get("kumi_call")
        ident = call.tool_call_id if isinstance(call, calls.Entry) else kwargs.get("tool_call_id")
        return {"profile_id": self.profile(),
                "session_id": kwargs.get("session_id") or None,
                "task_id": kwargs.get("task_id") or None,
                "tool_call_id": ident if isinstance(ident, str) and ident else None,
                "request_id": uuid.uuid4().hex}

    def on_session_finalize(self, session_id: str | None = None, **kwargs: Any) -> None:
        """The chat ended: stop the runtime it started (its DAW stays open)."""
        self.runtimes.stop_session(session_id)


# Hermes's surfaces where nobody answers an approval: webhooks, and its OpenAI-compatible API server, whose callers
# can't answer one either (Desktop's chats are its own surface, `desktop`).
UNATTENDED_SURFACES = frozenset({"webhook", "msgraph_webhook", "api_server"})
# What a scheduled job may call: reads answered from what Kumi last published, never from the runtime, whose every
# call makes its caller the runtime's holder (C3's owner).
SCHEDULED_READS = frozenset({"kumi_status", "kumi_history"})
SCHEDULED = ("A scheduled job reads only Kumi's status and HISTORY, as Kumi last published them: it never starts or "
             "calls Kumi's runtime, or changes anything. Open a chat with Kumi for that.")


def _session_values(*names: str) -> list[str]:
    """Hermes's session variables as Hermes reads them (its public `get_session_env`: the calling context's value,
    else the process environment), or the environment alone when Hermes isn't importable."""
    try:
        from gateway.session_context import get_session_env  # Hermes's
    except Exception:
        return [os.environ.get(name, "") for name in names]
    return [str(get_session_env(name, "") or "") for name in names]


def _truthy(value: str) -> bool:
    return value.strip().lower() in ("1", "true", "yes", "on")  # Hermes's own truthy words



def broadcast(event: str, payload: dict) -> None:
    """Tell this process's Desktop clients a snapshot changed (C5: `plugin.kumi.<event>`)."""
    from hermes_cli.plugin_events import broadcast_plugin_event  # Hermes's, documented
    broadcast_plugin_event(PLUGIN, event, payload)


SOUNDS_FOUND = 200
LISTEN_EVENT_BYTES = 256 * 1024


def announce(host: Host, tool: str, result: Any, *, status: Any, session_id: Any, tool_call_id: Any) -> None:
    """CC-9: after Hermes has a `kumi_sounds` or `kumi_listen` result, tell Desktop, so the sound map and the listen
    card work with tool progress off. Only a call Hermes reports `ok` with a result the model got (already without
    paths, CC-4, and without a failed listen's measurements, R5): `sounds.found` with up to 200 sound ids, and
    `listen.done` with the result itself when it's at most 256 KiB. CC-13's `generate.updated` carries a quote's, a
    create's or a generation job's card (13a: MIDI `notes` left out, none over 256 KiB). A failed broadcast never
    fails the call."""
    if status != "ok" or tool not in ("kumi_sounds", "kumi_listen", "kumi_generate", "kumi_job") \
            or not isinstance(result, str):
        return
    try:
        data = json.loads(result)
    except ValueError:
        return
    generation_job = tool == "kumi_job" and isinstance(data, dict) and data.get("kind") == "generate"
    if not isinstance(data, dict) or ("error" in data and not generation_job):  # a job's error sits beside its result
        return
    base = {"profile": host.profile(), "session_id": session_id, "tool_call_id": tool_call_id}
    if tool == "kumi_sounds":
        hits = [item for item in data.get("items") or []
                if isinstance(item, dict) and isinstance(item.get("sound_id"), str)][:SOUNDS_FOUND]
        if not hits:
            return
        event, payload = "sounds.found", found(base, data, hits)
    elif tool in ("kumi_generate", "kumi_job"):
        if tool == "kumi_job" and not generation_job:
            return
        card = generate.card(data)
        fits = len(json.dumps(card, ensure_ascii=False).encode("utf-8")) <= generate.CARD_BYTES
        event, payload = "generate.updated", {**base, "tool": tool, **({"card": card} if fits else {})}
    else:
        if not isinstance(data.get("metrics"), dict) or data.get("state") in ("refused", "cancelled", "unsure"):
            return
        fits = len(result.encode("utf-8")) <= LISTEN_EVENT_BYTES
        event, payload = "listen.done", {**base, **({"result": data} if fits else {})}
    try:
        broadcast(event, payload)
    except Exception:
        log.exception("Kumi couldn't tell Desktop about %s", tool)


def found(base: dict, data: dict, hits: list[dict]) -> dict:
    """CC-23's `sounds.found`: CC-9's ids for the map, and per hit `{sound_id, why, closeness?}` under `matches`, with
    `indexed` and the `near` seed. Never a name or a path. Past 256 KiB the reasons go first; the ids always stay."""
    payload = {**base, "map_revision": data.get("map_revision"), "sound_ids": [hit["sound_id"] for hit in hits]}
    for key in ("indexed", "near"):
        if key in data:
            payload[key] = data[key]
    for with_why in (True, False):
        matches = [{"sound_id": hit["sound_id"],
                    **({"why": hit["why"]} if with_why and isinstance(hit.get("why"), list) else {}),
                    **({"closeness": hit["closeness"]} if isinstance(hit.get("closeness"), int) else {})}
                   for hit in hits]
        if len(json.dumps({**payload, "matches": matches}, ensure_ascii=False).encode("utf-8")) <= LISTEN_EVENT_BYTES:
            return {**payload, "matches": matches}
    return payload


# What runs without asking first when Hermes's approvals are off: every call Kumi asks about (policy.py). One list,
# said the same everywhere (research-1's copy review, P3). Paid generations and their uploads are among them: only a
# turn with nobody in it refuses them (policy._paid, policy._upload), and the credit cap bounds what they spend (#1079).
UNASKED = ("Kumi's changes, undos, track listens, sample-folder adds and scans, setup steps, and paid Mirelo generations "
           "and the files they send to Mirelo (up to the credit cap)")
NO_APPROVALS = (f"Hermes's approvals are off here (--yolo, /yolo or approvals.mode: off), so {UNASKED} run without "
                "asking first.")
UNKNOWN_APPROVALS = ("Kumi can't tell whether /yolo is on in this chat (Hermes runs Kumi in its plugin isolation, or "
                     f"doesn't say). If it is, {UNASKED} run without asking first.")
# Whether Kumi asks before its changes ("Ask before each change"), said the same in /kumi, `hermes kumi status` and the
# doctor.
CHANGES_UNASKED = ("Kumi makes your changes and runs your scripts without asking (Ask before each change: off); setup "
                   "steps, sample folders and Mirelo jobs still ask")
CHANGES_ASKED = "Hermes asks before each Kumi change (Ask before each change: on)"


def changes_mode(host: Host) -> str:
    return CHANGES_ASKED if host.ask_before_changes() else CHANGES_UNASKED


# Set in the environment of Hermes's plugin host (`plugins.isolation: host`), whose process holds no chat's /yolo.
PLUGIN_HOST_ENV = "HERMES_PLUGIN_HOST_PROCESS"


def approval_gaps(host: Host) -> list[str]:
    """Hermes settings under which Kumi's approvals pass with nobody asked (research-1's review, item 8), each read as
    Hermes v0.22.7 reads it (`tools.approval_context._binary_approval_mode`, `utils.is_truthy_value`). Kumi itself
    refuses what needs one in scheduled jobs, webhooks and API calls (#983); `hermes -z`, which Hermes treats as
    --yolo, has no public sign Kumi can read (the README says so)."""
    config = host.hermes_config()
    approvals = config.get("approvals") if isinstance(config.get("approvals"), dict) else {}
    delegation = config.get("delegation") if isinstance(config.get("delegation"), dict) else {}
    gaps = []
    if str(approvals.get("single_query_mode", "deny")).lower().strip() in {"approve", "off", "allow", "yes"}:
        gaps.append(f"approvals.single_query_mode lets a one-shot hermes -q run {UNASKED} without asking first.")
    auto = delegation.get("subagent_auto_approve", False)
    if auto.strip().lower() in {"1", "true", "yes", "on"} if isinstance(auto, str) else bool(auto):
        gaps.append(f"delegation.subagent_auto_approve lets a subagent run {UNASKED} without asking first.")
    return gaps


def approvals_bypassed(host: Host, *, chat: bool = True) -> bool | None:
    """Whether Hermes runs a call it would ask about without asking, or None when Kumi can't tell:
    - `approvals.mode: off` in Hermes's settings (YAML's bare `off` reads as false);
    - `HERMES_YOLO_MODE` set for this process (what `--yolo` sets);
    - for a chat (`chat`), that chat's `/yolo`. Hermes has no documented way to read it, so this is a stated coupling
      with Hermes v0.22.7's `tools.approval.is_current_session_yolo_enabled` (catalog rule 9 warns of it). In Hermes's
      plugin host, which holds no chat's /yolo, or when that check is gone or fails, it's None: never a false "asks".
    A terminal (`chat=False`) can't see any chat's `/yolo`, and says so itself."""
    approvals = host.hermes_config().get("approvals")
    mode = approvals.get("mode") if isinstance(approvals, dict) else None
    if mode is False or (isinstance(mode, str) and mode.strip().lower() == "off"):
        return True
    if os.environ.get("HERMES_YOLO_MODE", "").strip().lower() in ("1", "true", "yes", "on"):
        return True
    if not chat:
        return False
    if os.environ.get(PLUGIN_HOST_ENV) == "1":
        return None
    try:
        from tools.approval import is_current_session_yolo_enabled  # Hermes's; see above
        return bool(is_current_session_yolo_enabled())
    except Exception:
        return None


def kumi_status(host: Host, args: dict, **kwargs: Any) -> dict:
    """Where Kumi stands, for the model: setup as `setup.for_model` gives it (no path on this computer, LOW 11), the
    approvals note, whether Kumi asks before its changes (`ask_before_changes`), and the runtime's own status."""
    daw = host.daw()
    data_dir = host.data_dir()
    report: dict[str, Any] = {"setup": setup.for_model(setup.status(daw=daw, data_dir=data_dir,
                                                                    plugin_dir=host.plugin_dir)),
                              "runtime": None, "connected": False, "daw": daw, "project": None, "owner": None,
                              "capabilities": [], "ask_before_changes": host.ask_before_changes()}
    bypassed = approvals_bypassed(host)
    said = ([] if bypassed is False else [NO_APPROVALS if bypassed else UNKNOWN_APPROVALS]) + approval_gaps(host)
    if said:  # say it to the producer before changing anything
        report["approvals"] = " ".join(said)
    try:
        host.install(data_dir)
    except runtime.Unavailable as failure:
        report["message"] = report["setup"].get("message") or setup.reason_for_model(failure.code, failure.reason)
        return report
    if host.unattended() == "scheduled":  # never the runtime: a call makes its caller the runtime's holder
        published = snapshots.status(Path(data_dir) / "state")
        report.update({key: published[key] for key in ("connected", "daw", "project")
                       if published.get(key) is not None})
        report["message"] = ("As Kumi last published it, for a scheduled job: " + (
            "Kumi's runtime is running." if published.get("running") is not False and not published.get("stale")
            else "Kumi's runtime isn't running; it starts when a chat connects to the DAW."))
        return report
    live = host.runtimes.get(data_dir)
    if live is None:
        settling = host.runtimes.settling(data_dir)
        if settling is not None:  # stopping: it holds the profile until it has settled and exited
            published = snapshots.owner(Path(data_dir) / "state").get("owner")  # the runtime's last holder (C3)
            report.update({"running": True, "settling": True, "daw": settling.daw, "message": runtime.BUSY,
                           "owner": published if isinstance(published, dict) else None})
            return report
        report["message"] = "Kumi's runtime isn't running: kumi_connect starts it and connects to the DAW."
        return report
    status = live.call("status", {}, host.identity(kwargs), timeout=STATUS_TIMEOUT)
    report.update({key: status[key] for key in ("runtime", "connected", "daw", "project", "owner", "capabilities")
                   if key in status})
    return report


def setup_daw_caller(host: Host, daw: str, kwargs: dict) -> Callable[[dict], dict]:
    """The runtime's `setup_daw` (C8's DAW step) for setup.py: this profile's runtime for `daw`, started for this chat
    when none runs. A runtime that works in another DAW refuses rather than being restarted under a running chat."""

    def call(args: dict) -> dict:
        data_dir = host.data_dir()
        install = host.install(data_dir)
        live = host.runtimes.get(data_dir)
        if live is not None and live.daw != daw:
            raise runtime.Refused("other_daw", (
                f"Kumi is running for {schemas.DAW_NAMES[live.daw]} in this profile. To set Kumi up for "
                f"{schemas.DAW_NAMES[daw]}, end the chat using Kumi, then set it up again."))
        live = live or host.runtimes.start(data_dir, install, daw, kwargs.get("session_id"),
                                           library_roots=host.library_folders(), media_roots=host.media_roots(),
                                           profile=host.profile())
        timeout = READ_TIMEOUT if args.get("dry_run") else SETUP_TIMEOUT
        return live.call("setup_daw", args, host.identity(kwargs), timeout=timeout)

    return call


def kumi_setup(host: Host, args: dict, **kwargs: Any) -> dict:
    """C8 through C1's kumi_setup. `plan` records the next step. `install` and `adopt` carry out exactly the plan the
    producer approved: the approval hook bound its digest to this call's record (CC-5), and only that plan runs. A
    digest in the model's own arguments is never used. What the model gets names no path or URL (LOW 11): the plan,
    the setup state and a failure's reason go through setup's model projections; the approval and `hermes kumi
    setup` show the producer every path."""
    action, daw = args.get("action"), args.get("daw") or host.daw()
    if action not in ("plan", "install", "adopt"):
        return {"error": {"code": "invalid", "reason": "action is plan, install or adopt."}}
    if daw not in schemas.DAWS:
        return {"error": {"code": "invalid", "reason": f"Kumi works in {', '.join(schemas.DAWS)}, not {daw!r}."}}
    data_dir = host.data_dir()
    setup_daw = setup_daw_caller(host, daw, kwargs)
    current = host.daw()  # the plugin's DAW setting: a DAW set up first becomes it, in the plan's own approval
    if action == "plan":
        try:
            planned = setup.plan(daw, setup_daw=setup_daw, data_dir=data_dir, plugin_dir=host.plugin_dir,
                                 current=current)
        except setup.SetupError as error:
            return {"error": {"code": error.code, "reason": setup.reason_for_model(error.code, error.message)}}
        next_action = {"runtime": "install", "daw_component": "install", "owned_elsewhere": "adopt",
                       "use_daw": "install",
                       "ffmpeg": "install (optional: ask the producer; setup is done either way)"}.get(planned["step"])
        shown = {**setup.plan_for_model(planned), "next": f"kumi_setup {next_action}" if next_action else None}
        if planned["step"] == "none" and setup.next_daw(daw, current):  # set up, but another DAW stays Kumi's
            shown["next"] = (setup.next_daw(daw, current) + " The producer runs it in a terminal, or changes the DAW "
                             "setting under Settings → Plugins → Kumi in Hermes Desktop.")
        return shown
    call = kwargs.get("kumi_call")
    digest = call.extra.get("digest") if isinstance(call, calls.Entry) else None
    if not isinstance(digest, str) or not digest:
        return {"error": {"code": "not_approved", "reason": "Kumi didn't set anything up: this call carries no "
                                                            "approved setup plan. Run kumi_setup plan first."}}
    if action == "install":
        result = setup.install(daw, expected_digest=digest, setup_daw=setup_daw, data_dir=data_dir,
                               plugin_dir=host.plugin_dir, current=current)
    else:
        result = setup.adopt(daw, expected_digest=digest, setup_daw=setup_daw, data_dir=data_dir,
                             plugin_dir=host.plugin_dir, current=current)
    if not result.get("ok"):
        error = result.get("error") or {}
        code = error.get("code") or "failed"
        return {"error": {"code": code, "reason": setup.reason_for_model(code, error.get("message"))}}
    if result.get("use_daw") == daw:  # the producer approved making it Kumi's DAW (the runtime already works in it)
        name = schemas.DAW_NAMES[daw]
        try:
            host.set_daw(daw)
            said = f"{name} is Kumi's DAW in this profile now."
        except Exception:
            log.warning("Kumi couldn't change its DAW setting")
            said = (f"Kumi couldn't change its DAW setting, so it still connects to {schemas.DAW_NAMES[current]}: "
                    f"the producer sets it to {daw} in Hermes's settings for the Kumi plugin.")
            result = {key: value for key, value in result.items() if key != "use_daw"}
        result = {**result, "message": f"{result['message']} {said}".strip()}
    if action == "adopt" or result.get("step") in ("runtime", "ffmpeg"):
        host.runtimes.stop(data_dir)  # the next start uses the new runtime, the adopted --kumi-home or the ffmpeg
    return {**result, "status": setup.for_model(result.get("status") or {})}


def kumi_connect(host: Host, args: dict, **kwargs: Any) -> dict:
    daw = args.get("daw") or host.daw()
    if daw not in schemas.DAWS:
        return {"error": {"code": "invalid", "reason": f"Kumi works in {', '.join(schemas.DAWS)}, not {daw!r}."}}
    data_dir = host.data_dir()
    install = host.install(data_dir)
    live = host.runtimes.get(data_dir)
    if live is not None and live.daw != daw:
        return {"error": {"code": "other_daw", "reason": (
            f"Kumi is running for {schemas.DAW_NAMES[live.daw]} in this profile. To use {schemas.DAW_NAMES[daw]}, "
            "end the chat using Kumi, then connect again.")}}
    live = live or host.runtimes.start(data_dir, install, daw, kwargs.get("session_id"),
                                       library_roots=host.library_folders(), media_roots=host.media_roots(),
                                       profile=host.profile())
    return live.call("connect", {"daw": daw}, host.identity(kwargs), timeout=CONNECT_TIMEOUT)


def _forward(tool: str, timeout: float) -> Callable[..., dict]:
    """A read tool: the runtime's tool of the same name (C2), with the model's arguments as they are."""

    def handler(host: Host, args: dict, **kwargs: Any) -> dict:
        return host.live().call(tool, args, host.identity(kwargs), timeout=timeout)

    handler.__name__ = f"kumi_{tool}"
    return handler


def plugin_knowledge_refusal(args: dict) -> str | None:
    """Keep this read separate from the mixed plug-in/preset/wavetable dispatchers."""
    action = args.get("action")
    if action not in ("guide", "catalog"):
        return "kumi_plugin_knowledge's action is guide or catalog."
    allowed = {"action", "max_chars", "device_ref"} if action == "guide" else {"action", "max_chars", "plugin", "query", "limit"}
    if args.keys() - allowed:
        return "guide takes only device_ref; catalog takes only plugin, query and limit; either may take max_chars. No file or writer fields."
    if action == "guide":
        device = args.get("device_ref")
        if not isinstance(device, str) or not device.strip() or len(device) > 256:
            return "guide needs the Live plug-in's device_ref from the current look."
    else:
        for field, minimum, maximum in (("plugin", 1, 128), ("query", 0, 1000)):
            if field in args and (not isinstance(args[field], str) or not minimum <= len(args[field]) <= maximum):
                return f"{field} must be text of {minimum}–{maximum} characters."
        if "limit" in args and (type(args["limit"]) is not int or not 1 <= args["limit"] <= 40):
            return "limit is an integer from 1 to 40."
    if "max_chars" in args and (type(args["max_chars"]) is not int or not 500 <= args["max_chars"] <= 20000):
        return "max_chars is an integer from 500 to 20000."
    return None


def kumi_plugin_knowledge(host: Host, args: dict, **kwargs: Any) -> dict:
    refusal = plugin_knowledge_refusal(args)
    if refusal is not None:
        return {"error": {"code": "invalid", "reason": refusal}}
    if args["action"] == "guide":
        live = host.live()
        if live.daw != "ableton":
            return {"error": {"code": "unsupported", "reason": (
                "Instance guides are available in Live; use catalog for bundled format knowledge and kumi_read "
                "for this DAW's actual parameters.")}}
        sent = {"action": "guide", "device_ref": args["device_ref"]}
        if "max_chars" in args:
            sent["max_chars"] = args["max_chars"]
    else:
        data_dir = host.data_dir()
        install = host.install(data_dir)
        live = host.runtimes.get(data_dir) or host.runtimes.start(
            data_dir, install, host.daw(), kwargs.get("session_id"),
            library_roots=host.library_folders(), media_roots=host.media_roots(), profile=host.profile())
        sent = {"action": "catalog", **{key: args[key] for key in ("plugin", "query", "limit", "max_chars") if key in args}}
    return live.call("plugin_knowledge", sent, host.identity(kwargs), timeout=READ_TIMEOUT)


def _write(host: Host, tool: str, args: dict, kwargs: dict, timeout: float | None = None,
           remember: bool = True) -> dict:
    """A write (C2): sent once. An answer that never came, or can't be read, means the outcome is unknown: reported as
    unsure, never resent, and never called unchanged. The producer's Stop cancels it while it waits (`notifications/
    cancelled`), so its remaining steps don't go on applying after Hermes has given the call up (research-1's review,
    item 7); that's unsure too."""
    live = host.live()
    try:
        answer = live.call(tool, args, host.identity(kwargs), timeout=timeout or WRITE_TIMEOUT,
                           interrupted=host.stop_watch())
    except runtime.Unavailable as failure:
        if failure.code not in UNSURE_CODES:
            raise
        return {"state": "unsure", "code": failure.code, "reason": failure.reason, "hint": UNSURE_HINT}
    if tool != "undo" and remember:  # every other write that made rows is the chat's newest change (CC-3 A2)
        host.remember_change(kwargs, answer)
    return answer


def kumi_script(host: Host, args: dict, **kwargs: Any) -> dict:
    """CC-19: a script, run once as the producer approved it. The hook's record holds only the arguments it was asked
    about (CC-5), so the code here is the approved code, and its SHA-256 tells the runtime so: a runtime that would
    run anything else refuses it (`stale`)."""
    script = {key: args[key] for key in ("language", "purpose", "code", "expected_project_key") if key in args}
    code = script.get("code")
    if isinstance(code, str):
        script["expected_sha256"] = scripts.sha256(code)
    return _write(host, "script", script, kwargs)


# A device build whose answer never came: a file may be in the User Library, and building again would write another.
DEVICE_UNSURE_HINT = ("Whether the device was written is unknown. Don't build it again: ask the producer to look in "
                      "User Library/Kumi/Audio Effects in Live's Browser.")


def kumi_device(host: Host, args: dict, **kwargs: Any) -> dict:
    """CC-22: a Max for Live audio effect, built once as the producer approved it. The spec is written in its one
    canonical form, the bytes the approval hashed: serve hashes what it receives and refuses a build whose spec isn't
    the approved SHA-256 (`stale`), or whose file isn't the one the approval named. A refusal or a stale answer goes
    back as serve gave it; a built device comes with the next step, loading and placing it in a change of its own."""
    call = kwargs.get("kumi_call")
    approved = call.extra.get("sha256") if isinstance(call, calls.Entry) else None
    spec = args.get("spec")
    spec_json = devices.canonical(spec) if devices.problem(spec) is None else None
    if spec_json is None or not isinstance(approved, str) or scripts.sha256(spec_json) != approved:
        return {"error": {"code": "not_approved", "reason": "Kumi didn't build the device: it isn't the spec the "
                                                            "producer approved."}}
    built = _write(host, "device", {"action": "build", "spec_json": spec_json, "sha256": approved}, kwargs)
    if built.get("state") == "unsure":
        return {**built, "hint": DEVICE_UNSURE_HINT}
    if isinstance(built.get("item"), str):
        host.remember_library_write(kwargs, built.get("file"))
        return {**built, "next": devices.next_step(built)}
    return built


# What a build takes, as serve's preset_build schema does: anything else is refused before serve is asked (agent-m's
# composition smoke: a version the model added was dropped, and the build went ahead).
PRESET_BUILD_FIELDS = frozenset({"plugin", "class", "name", "edits"})
PRESET_BUILD_FIELDS_ONLY = ("Kumi didn't build the preset: it takes only plugin, class, name and edits, never a file, "
                            "bytes or a version of its own.")


def kumi_preset_build(host: Host, args: dict, **kwargs: Any) -> dict:
    """CC-26: a preset built from the verified writer's initialized template and the model's typed edits, kept only as
    Kumi's private copy (an artifact): nothing reaches Live or the User Library until kumi_preset is approved. It runs
    unasked in the producer's own turn, never unattended, forked or scheduled (policy, 26-w1)."""
    if not set(args) <= PRESET_BUILD_FIELDS:  # never filtered away: a version, file or bytes the model adds is refused
        return {"error": {"code": "invalid", "reason": PRESET_BUILD_FIELDS_ONLY}}
    return host.live().call("preset_build", dict(args), host.identity(kwargs), timeout=WRITE_TIMEOUT)


DESIGN_FIELDS = frozenset({"op", "mode", "sources", "seed", "options"})
DESIGN_FIELDS_ONLY = ("Kumi didn't make the sound: kumi_design takes op, mode, sources (by sound_id and map_revision, or "
                      "artifact_id), seed and options, never a file.")


def kumi_design(host: Host, args: dict, **kwargs: Any) -> dict:
    """A new sound crossed, fused or stacked from 2-16 sounds (serve's `design`), kept as Kumi's private artifact:
    nothing in the DAW changes. Sources by id only, never a file the model names (CC-4); a sound the dashboard made
    goes in as its kept file (`runtime.designs_as_files`). It runs unasked in the producer's own turn, never
    unattended, forked or scheduled (policy). Its MEDIA: line passes only for its kept file. The answer names the
    sources as the model gave them, so a dashboard design stays its id."""
    sources = args.get("sources")
    if not set(args) <= DESIGN_FIELDS or not isinstance(sources, list) or any(
            not isinstance(source, dict) or "file" in source for source in sources):
        return {"error": {"code": "invalid", "reason": DESIGN_FIELDS_ONLY}}
    made = kept_media(host, host.live().call("design", dict(args), host.identity(kwargs), timeout=WRITE_TIMEOUT))
    return {**made, "sources": sources} if "sources" in made else made


# A place whose answer never came: the file may be in the User Library. The same preset always names the same file, and
# placing it again answers that file without writing another (#1180), so asking again is safe.
PRESET_UNSURE_HINT = ("Whether the preset was written is unknown. Ask for it again with kumi_preset: placing the same "
                      "preset never writes a second file, it answers the one already there.")


def kumi_preset(host: Host, args: dict, **kwargs: Any) -> dict:
    """CC-26: the preset the producer approved, placed as serve's preview named it: the approval's digest and file go
    with it, so serve writes exactly that file of exactly those bytes, or refuses `changed` and writes nothing (26-r1).
    A written file is this chat's newest write, which "undo that" never undoes (26-w5)."""
    call = kwargs.get("kumi_call")
    approved = call.extra if isinstance(call, calls.Entry) else {}
    artifact = args.get("artifact_id")
    if (approved.get("artifact_id") != artifact or not isinstance(approved.get("digest"), str)
            or not isinstance(approved.get("file"), str)):
        return {"error": {"code": "not_approved", "reason": "Kumi didn't put the preset in your User Library: it isn't "
                                                            "the one the producer approved."}}
    placed = _write(host, "preset", {"action": "place", "artifact_id": artifact, "digest": approved["digest"],
                                     "file": approved["file"]}, kwargs)
    if placed.get("state") == "unsure":
        return {**placed, "hint": PRESET_UNSURE_HINT}
    if isinstance(placed.get("file"), str) and "error" not in placed:
        host.remember_library_write(kwargs, placed["file"])
    return placed



def kumi_change(host: Host, args: dict, **kwargs: Any) -> dict:
    change = {key: value for key, value in args.items() if key != "preview"}  # previews are the approval hook's (CC-3)
    if "expected_epoch" in change:
        change["expected_epoch"] = str(change["expected_epoch"])
    answer = _write(host, "change", change, kwargs)
    note = copy_note(answer.get("copy"), host.daw()) if isinstance(answer, dict) else None
    return {**answer, "copy_note": note} if note else answer


# The copy of the project a plan keeps beside it before a step that could lose material (Live's before its third step
# or a delete; Renoise's before a sample load), as serve's change answer gives it (`copy`: the file's name and why;
# absent when none was kept). Told to the model as standalone Kumi tells it, by the DAW's own word for the project and
# the file's name: serve keeps the producer's paths out of its answers.
COPY_WHY = {"third_step": "before the plan's third step", "deletes": "before a step that could lose material"}
PLACES = {"ableton": "Set", "reaper": "project", "renoise": "song"}


def copy_note(copy: Any, daw: str) -> str | None:
    """What the model tells the producer about the copy of their Set, project or song this change kept. A copy the DAW
    couldn't keep never reaches serve (Live's keep_copy answers nothing then), so there's no failure to tell."""
    file = _copy_file(copy)
    if file is None:
        return None
    why = COPY_WHY.get(copy.get("why"))
    return (f"Before this, Kumi kept a copy of the producer's {PLACES.get(daw, 'project')} as last saved, next to it: "
            f"{file}{f' ({why})' if why else ''}. Tell the producer in a few words, with the file's name.")


def _copy_file(copy: Any) -> str | None:
    """The kept copy's file name: only its last part, even if a path were ever sent, so no path shows."""
    file = copy.get("file") if isinstance(copy, dict) else None
    name = re.split(r"[/\\]", file)[-1].strip() if isinstance(file, str) else ""
    return _words(name, 120) if name else None


def _words(text: str, limit: int) -> str:
    """`text` as one line of at most `limit` characters, without control or invisible format characters."""
    line = " ".join("".join(char for char in text if char.isprintable()).split())
    return line if len(line) <= limit else line[:limit - 1].rstrip() + "…"


# kumi_undo with no id, where this chat made no change Kumi remembers (CC-3 amendment A2), or where its newest step
# wrote a file into the User Library, which Kumi never undoes (26-w5); and how many of a chat's writes Kumi keeps for it.
NOTHING_TO_UNDO = ("Kumi didn't undo anything: this chat made no change Kumi remembers since Hermes started, or it "
                   "undid them all. kumi_history lists Kumi's changes; undo one by its id.")
FILE_STAYS = ("Kumi didn't undo anything: this chat's newest step wrote {file}, a file Kumi never undoes (it stays, as "
              "its approval said). kumi_history lists Kumi's changes; undo one by its id.")
CHANGES_KEPT = 20


def undo_target(host: Host, args: dict, kwargs: dict) -> tuple[dict | None, str | None]:
    """What a kumi_undo call undoes: with a history_id, that row (or its whole change, with whole_change: true);
    without one, this chat's newest change, whole (CC-3 amendment A2). The runtime's undo arguments, or a refusal."""
    asked = {key: value for key, value in args.items() if key in ("history_id", "whole_change")}
    if "history_id" in asked:
        return asked, None
    newest = host.newest_write(kwargs)
    if newest is None:
        return None, NOTHING_TO_UNDO
    kind, target = newest
    if kind == "file":
        return None, FILE_STAYS.format(file=target)
    return {"history_id": target, "whole_change": True}, None


def kumi_undo(host: Host, args: dict, **kwargs: Any) -> dict:
    """An undo, as the producer approved it (CC-3, A2): the row or change the hook previewed. A whole change is sent with
    the preview's rows digest (`expect_rows`), so the runtime refuses a set that changed since; and kumi_undo with no
    id is refused when this chat's newest change is no longer the one approved."""
    asked, refusal = undo_target(host, args, kwargs)
    if refusal is not None:
        return {"error": {"code": "nothing_to_undo", "reason": refusal}}
    if asked.get("whole_change") is True:
        call = kwargs.get("kumi_call")
        approved = call.extra if isinstance(call, calls.Entry) else {}
        if approved.get("history_id") != asked["history_id"] or not isinstance(approved.get("rows_digest"), str):
            return {"state": "refused", "code": "stale",
                    "reason": "Kumi didn't undo anything: what this chat would undo isn't what was approved (a newer "
                              "change came since). Ask again."}
        asked = {**asked, "expect_rows": approved["rows_digest"]}
    answer = _write(host, "undo", asked, kwargs)
    host.remember_undo(asked, answer)
    return answer


def kumi_variations(host: Host, args: dict, **kwargs: Any) -> dict:
    """CC-24: list reads the set. make and change run the takes the producer approved: the hook's preview drew them
    from a seed, and the dispatch sends that seed with the preview's takes digest, so the runtime writes those takes or
    nothing (`stale`). keep renames as approved."""
    asked = {key: value for key, value in args.items() if key != "preview"}
    if args.get("action") == "list":
        return host.live().call("variations", asked, host.identity(kwargs), timeout=READ_TIMEOUT)
    if args.get("action") in ("make", "change"):
        call = kwargs.get("kumi_call")
        drawn = call.extra if isinstance(call, calls.Entry) else {}
        if not isinstance(drawn.get("seed"), int) or not isinstance(drawn.get("takes_digest"), str):
            return {"error": {"code": "not_approved", "reason": "Kumi didn't make any takes: no approval saw them."}}
        asked |= {"seed": drawn["seed"], "expect_takes": drawn["takes_digest"]}
    return _write(host, "variations", asked, kwargs)


# Judged listening (serve's judge and tune) and wavetables, Live only. No runtime preview exists for them, so the
# plugin checks their arguments itself before anything is asked or sent, in the hook and again in the handler.
JUDGED_TIMEOUT = 900.0  # a round plays at most 60 s a pass; a tune's search or probe hears several passes
TUNE_HOWS = ("fit", "home", "search", "probe")
TUNE_FIELDS = frozenset({"device", "how", "target", "knobs", "change", "expected_project_key", "expected_epoch"})
JUDGE_FIELDS = frozenset({"goal", "track", "from_beat", "beats", "change", "done", "expected_project_key",
                          "expected_epoch"})
WAVETABLE_UNSURE_HINT = ("Whether the wavetable was written is unknown. Don't make it again: ask the producer to look "
                         "in the plug-in's wavetable menu.")


def _expected_refusal(args: dict) -> str | None:
    key, epoch = args.get("expected_project_key"), args.get("expected_epoch")
    if not isinstance(key, str) or not key.strip():
        return "it needs expected_project_key, the project key kumi_observe gave."
    if not ((type(epoch) is int and epoch >= 0) or (isinstance(epoch, str) and epoch.isascii() and epoch.isdigit())):
        return "it needs expected_epoch, the epoch kumi_observe gave."
    return None


def _number(value: Any) -> bool:
    return type(value) in (int, float) and _finite(value)


def judge_refusal(args: dict) -> str | None:
    """Why serve's judge wouldn't take these arguments, or None: the project key and epoch, and exactly one of goal
    (with from_beat and beats, a bounded stretch), change or done: true; a reference is {clip_ref} or a source, never a
    path string."""
    if not set(args) <= JUDGE_FIELDS | {"preview"}:
        return f"it takes only {', '.join(sorted(JUDGE_FIELDS))}."
    given = [key for key in ("goal", "change", "done") if key in args]
    if len(given) != 1:
        return "give one of goal (with from_beat and beats), change, or done: true."
    if "done" in args and args["done"] is not True:
        return "done is true, to end the run."
    if "goal" not in args and TRACK_KEYS & set(args):
        return "track, from_beat and beats go with a goal."
    if "change" in args and (not isinstance(args["change"], str) or not args["change"].strip()):
        return "change is what you changed since the last round, in one line."
    if "goal" in args:
        goal = args["goal"]
        if not isinstance(goal, dict):
            return "goal is an object: what to reach."
        start, beats = args.get("from_beat"), args.get("beats")
        if not _number(start) or start < 0 or not _number(beats) or not 0 < beats <= MAX_BEATS:
            return f"a goal needs from_beat (0 or more) and beats (above 0, at most {MAX_BEATS}): the stretch it hears."
        if "track" in args and (not isinstance(args["track"], str) or not args["track"].strip()):
            return "track is one track's ref or name; leave it out for the mix."
        if "reference" in goal and not _judge_reference(goal["reference"]):
            return ("a reference is {\"clip_ref\"} for an audio clip in the Set, or a source: {\"sound_id\", "
                    "\"map_revision\"}, {\"artifact_id\"} or {\"file\"} in an approved folder, never a path.")
    return _expected_refusal(args)


def _judge_reference(reference: Any) -> bool:
    if isinstance(reference, dict) and set(reference) == {"clip_ref"}:
        return isinstance(reference["clip_ref"], str) and bool(reference["clip_ref"].strip())
    return _file_source(reference)


def tune_refusal(args: dict) -> str | None:
    """Why serve's tune wouldn't take these arguments, or None: a device ref, how, the knobs how needs, and the
    project key and epoch."""
    if not set(args) <= TUNE_FIELDS | {"preview"}:
        return f"it takes only {', '.join(sorted(TUNE_FIELDS))}."
    if not isinstance(args.get("device"), str) or not args["device"].strip():
        return "name the device to tune by its ref."
    how = args.get("how")
    if how not in TUNE_HOWS:
        return "how is fit, home, search or probe."
    knobs = args.get("knobs", [])
    if not isinstance(knobs, list) or len(knobs) > 5 or not all(isinstance(k, str) and k.strip() for k in knobs):
        return "knobs is a list of up to 5 knob names."
    if how in ("home", "probe") and len(knobs) != 1:
        return "home and probe move one knob: give its name in knobs."
    if how == "search" and not 2 <= len(knobs) <= 5:
        return "search tunes 2 to 5 knobs that interact: give their names in knobs."
    for key in ("target", "change"):
        if key in args and (not isinstance(args[key], str) or not args[key].strip()):
            return f"{key} is a few words."
    return _expected_refusal(args)


def wavetable_refusal(args: dict) -> str | None:
    """Why serve's wavetable wouldn't take these arguments, or None: device_ref and wavetable only, and from_audio
    takes a source, never a file path."""
    if not set(args) <= {"device_ref", "wavetable", "preview"}:
        return "it takes only device_ref and wavetable."
    if not isinstance(args.get("device_ref"), str) or not args["device_ref"].strip():
        return "give device_ref, the plug-in device's ref from the current look."
    table = args.get("wavetable")
    if not isinstance(table, dict) or not table:
        return "give wavetable: keyframes (with count), or from_audio."
    if "from_audio" in table:
        cut = table["from_audio"]
        if not isinstance(cut, dict) or "file" in cut or not _file_source(cut.get("source")):
            return ("from_audio takes source: {\"sound_id\", \"map_revision\"}, {\"artifact_id\"} or {\"file\"} in an "
                    "approved folder, never a path of its own.")
    return None


def _judged(host: Host, tool: str, args: dict, kwargs: dict) -> dict:
    """A judge or tune round, sent once as cleared (the epoch as a string, as kumi_change sends it). Its rows are the
    chat's newest change; an earlier change the round took back is undone now, so "undo that" never means it."""
    refusal = (judge_refusal if tool == "judge" else tune_refusal)(args)
    if refusal is not None:
        return {"state": "refused", "code": "invalid", "reason": f"Kumi didn't run this: {refusal}"}
    asked = {key: value for key, value in args.items() if key != "preview"}
    asked["expected_epoch"] = str(asked["expected_epoch"])
    answer = _write(host, tool, asked, kwargs, timeout=JUDGED_TIMEOUT)
    taken = answer.get("taken_back") if isinstance(answer, dict) else None
    if isinstance(taken, list) and taken:
        host.remember_undo({}, {"rows": [{"history_id": each, "state": "undone"} for each in taken]})
    return answer


def kumi_judge(host: Host, args: dict, **kwargs: Any) -> dict:
    return _judged(host, "judge", args, kwargs)


def kumi_tune(host: Host, args: dict, **kwargs: Any) -> dict:
    return _judged(host, "tune", args, kwargs)


def kumi_wavetable(host: Host, args: dict, **kwargs: Any) -> dict:
    """A new wavetable file, written once as cleared. The file stays after an undo: it's this chat's newest write, which
    "undo that" never undoes."""
    refusal = wavetable_refusal(args)
    if refusal is not None:
        return {"error": {"code": "invalid", "reason": f"Kumi didn't make the wavetable: {refusal}"}}
    made = _write(host, "wavetable", {key: value for key, value in args.items() if key != "preview"}, kwargs)
    if made.get("state") == "unsure":
        return {**made, "hint": WAVETABLE_UNSURE_HINT}
    file, folder = made.get("file"), made.get("folder")
    if "error" not in made and isinstance(file, str) and file.strip():
        where = f" into {_words(folder, 80)}" if isinstance(folder, str) and folder.strip() else ""
        host.remember_library_write(kwargs, f"{_words(file, 120)}{where}")
    return made


# Live's other listening tools (serve's sound, audition, groove and render), Live only, checked here as judge is: each
# pass plays at most 60 s (a bar before the stretch and half a bar after included) through Kumi Ears with Live stopped,
# and a render is a minute of a track's own clips at most. sound, audition and render are listens; a groove round that
# changes notes (change, apply) is a change.
SOUND_FIELDS = frozenset({"tracks", "against", "key", "from_beat", "beats", "expected_project_key", "expected_epoch"})
AUDITION_FIELDS = frozenset({"candidates", "from_beat", "beats", "reference", "reference_from_seconds",
                             "reference_seconds", "focus", "expected_project_key", "expected_epoch"})
GROOVE_FIELDS = frozenset({"clip", "reference", "change", "apply", "amount", "done", "expected_project_key",
                           "expected_epoch"})
RENDER_FIELDS = frozenset({"track", "from_beat", "beats", "expected_project_key", "expected_epoch"})
REFERENCE_SECONDS = 60
_GROOVE_FILE = re.compile(r"[/\\]|^~|\.(mid|midi|wav|wave|aif|aiff|aifc|mp3|m4a|aac|mp4|flac|ogg|oga|opus|caf)$",
                          re.IGNORECASE)


def _text(value: Any, limit: int = 256) -> bool:
    return isinstance(value, str) and bool(value.strip()) and len(value) <= limit


def _stretch_refusal(args: dict, required: bool) -> str | None:
    start, beats = args.get("from_beat"), args.get("beats")
    if (required or start is not None) and (not _number(start) or start < 0):
        return "from_beat is where it starts, in beats (0 or more)."
    if beats is not None and (not _number(beats) or not 0 < beats <= MAX_BEATS):
        return f"beats is above 0 and at most {MAX_BEATS} (at most 60 s at the tempo, lead-in included)."
    return None


def sound_refusal(args: dict) -> str | None:
    """Why serve's sound wouldn't take these arguments, or None: 1-8 tracks, from_beat (beats optional), and the
    project key and epoch."""
    if not set(args) <= SOUND_FIELDS | {"preview"}:
        return f"it takes only {', '.join(sorted(SOUND_FIELDS))}."
    tracks = args.get("tracks")
    if not isinstance(tracks, list) or not 1 <= len(tracks) <= 8 or not all(_text(track) for track in tracks):
        return "tracks is 1 to 8 tracks, by ref or name."
    if "against" in args and not _text(args["against"]):
        return "against is a track, by ref or name."
    if "key" in args and not _text(args["key"], 40):
        return 'key is the song\'s key, like "F# minor".'
    return _stretch_refusal(args, True) or _expected_refusal(args)


def audition_refusal(args: dict) -> str | None:
    """Why serve's audition wouldn't take these arguments, or None: 1-8 candidates (a track, maybe a Session clip, or
    the mix alone), from_beat unless every candidate has its clip, a reference as {clip_ref} or a source (never a
    path), at most 60 s of it, and the project key and epoch."""
    if not set(args) <= AUDITION_FIELDS | {"preview"}:
        return f"it takes only {', '.join(sorted(AUDITION_FIELDS))}."
    candidates = args.get("candidates")
    if not isinstance(candidates, list) or not 1 <= len(candidates) <= 8:
        return "candidates is 1 to 8 tracks to hear, or the whole mix alone."
    for candidate in candidates:
        if not isinstance(candidate, dict) or not set(candidate) <= {"track", "mix", "clip", "label"}:
            return "each candidate is {track, clip?, label?} or {mix: true}."
        if candidate.get("mix") is True:
            if len(candidates) > 1 or "track" in candidate or "clip" in candidate:
                return "the whole mix is auditioned alone, with no track or clip."
        elif not _text(candidate.get("track")) or ("clip" in candidate and not _text(candidate["clip"])):
            return "each candidate names its track by ref (and a Session clip by its clip ref)."
        if "label" in candidate and not _text(candidate["label"], 60):
            return "a label is a few words."
    clips = all("clip" in candidate for candidate in candidates)
    refusal = _stretch_refusal(args, not clips)
    if refusal is not None:
        return refusal
    if "reference" in args and not _judge_reference(args["reference"]):
        return ("a reference is {\"clip_ref\"} for an audio clip in the Set, or a source: {\"sound_id\", "
                "\"map_revision\"}, {\"artifact_id\"} or {\"file\"} in an approved folder, never a path.")
    if "reference" not in args and ({"reference_seconds", "reference_from_seconds"} & set(args)):
        return "reference_seconds and reference_from_seconds go with a reference."
    seconds, start = args.get("reference_seconds"), args.get("reference_from_seconds")
    if seconds is not None and (not _number(seconds) or not 0 < seconds <= REFERENCE_SECONDS):
        return f"reference_seconds is above 0 and at most {REFERENCE_SECONDS}."
    if start is not None and (not _number(start) or start < 0):
        return "reference_from_seconds is 0 or more."
    if "focus" in args and args["focus"] not in ("sound", "section"):
        return "focus is sound or section."
    return _expected_refusal(args)


def groove_refusal(args: dict) -> str | None:
    """Why serve's groove wouldn't take these arguments, or None: a start (clip and a MIDI clip reference, never a
    file), change, apply (with amount 0-1) or done: true, and the project key and epoch."""
    if not set(args) <= GROOVE_FIELDS | {"preview"}:
        return (f"it takes only {', '.join(sorted(GROOVE_FIELDS))}: a reference is a MIDI clip's clip ref, so there's "
                "no audio stem or tempo to give.")
    start = "clip" in args or "reference" in args
    given = [start, "change" in args, "apply" in args, "done" in args]
    if sum(given) != 1:
        return "give one of clip with reference (a start), change, apply: true, or done: true."
    if start and not (_text(args.get("clip")) and _text(args.get("reference"))):
        return "a start gives clip (the part's clip ref) and reference (a MIDI clip's clip ref)."
    if start and _GROOVE_FILE.search(args["reference"]):
        return "the reference is a MIDI clip's clip ref, never a file."
    if "change" in args and not _text(args["change"], 400):
        return "change is what you changed in the part, in one line."
    for key in ("apply", "done"):
        if key in args and args[key] is not True:
            return f"{key} is true."
    if "amount" in args and ("apply" not in args or not _number(args["amount"]) or not 0 <= args["amount"] <= 1):
        return "amount goes with apply: how far, 0 to 1."
    return _expected_refusal(args)


def groove_changes(args: dict) -> bool:
    """Whether a groove call may change the part's notes: a change judged (and maybe taken back), or apply."""
    return "change" in args or args.get("apply") is True


def render_refusal(args: dict) -> str | None:
    """Why serve's render wouldn't take these arguments, or None: an audio track, from_beat and beats, and the
    project key and epoch."""
    if not set(args) <= RENDER_FIELDS | {"preview"}:
        return f"it takes only {', '.join(sorted(RENDER_FIELDS))}."
    if not _text(args.get("track")):
        return "track is an audio track's ref from the current look."
    if args.get("beats") is None:
        return "give from_beat and beats: at most 60 s of the track."
    return _stretch_refusal(args, True) or _expected_refusal(args)


LISTENING_REFUSALS = {"sound": sound_refusal, "audition": audition_refusal, "groove": groove_refusal,
                      "render": render_refusal}


def _listening(host: Host, tool: str, args: dict, kwargs: dict) -> dict:
    """A sound, audition, groove or render call, sent once as cleared (the epoch as a string). A groove round's rows
    are the chat's newest change, and a change it took back is undone now; a listen's (an audition's heard row) never
    are, so "undo that" never means a listen."""
    refusal = LISTENING_REFUSALS[tool](args)
    if refusal is not None:
        return {"state": "refused", "code": "invalid", "reason": f"Kumi didn't run this: {refusal}"}
    asked = {key: value for key, value in args.items() if key != "preview"}
    asked["expected_epoch"] = str(asked["expected_epoch"])
    answer = _write(host, tool, asked, kwargs, timeout=JUDGED_TIMEOUT, remember=tool == "groove")
    taken = answer.get("taken_back") if isinstance(answer, dict) else None
    if isinstance(taken, list) and taken:
        host.remember_undo({}, {"rows": [{"history_id": each, "state": "undone"} for each in taken]})
    return answer


def kumi_sound(host: Host, args: dict, **kwargs: Any) -> dict:
    return _listening(host, "sound", args, kwargs)


def kumi_audition(host: Host, args: dict, **kwargs: Any) -> dict:
    return _listening(host, "audition", args, kwargs)


def kumi_groove(host: Host, args: dict, **kwargs: Any) -> dict:
    return _listening(host, "groove", args, kwargs)


def kumi_render(host: Host, args: dict, **kwargs: Any) -> dict:
    return _listening(host, "render", args, kwargs)


# What a HISTORY row may say outside Kumi (C1, CC-12's 12a), live or published: never CC-5's internals or the runtime's
# own fields, and no `note`, whose error text can name a path (12f).
HISTORY_FIELDS = ("id", "title", "family", "at", "state", "undo_available", "track")


def kumi_history(host: Host, args: dict, **kwargs: Any) -> dict:
    """HISTORY (C1): from this process's runtime in a chat; as Kumi last published it (CC-12) when no runtime runs
    here, and always in a scheduled job, whose runtime call would make it the runtime's holder (12d). Either way, each
    row says only C1's fields. The published answer needs no setup check: its snapshot is there whatever setup says."""
    data_dir = host.data_dir()
    scheduled = host.unattended() == "scheduled"
    live = None if scheduled else host.runtimes.get(data_dir)
    if live is not None:
        answer = live.call("history", args, host.identity(kwargs), timeout=READ_TIMEOUT)
        rows = answer.get("items") if isinstance(answer.get("items"), list) else []
        return {**answer, "items": [_c1_row(row) for row in rows if isinstance(row, dict)]}
    return published_history(Path(data_dir) / "state", args.get("limit", 20), scheduled=scheduled)


def _c1_row(row: dict) -> dict:
    kept = {key: row[key] for key in HISTORY_FIELDS if key in row}
    copy = _row_copy(row.get("copy"))
    return {**kept, "copy": copy} if copy else kept


def _row_copy(copy: Any) -> dict | None:
    """The copy of the project a change kept, as a HISTORY row may say it outside Kumi: the file's name, never a path,
    and why."""
    if not isinstance(copy, dict):
        return None
    why = copy.get("why") if copy.get("why") in COPY_WHY else None
    file = _copy_file(copy)
    return {"file": file, "why": why} if file else None


def published_history(state: Path, limit: object, *, scheduled: bool) -> dict:
    """HISTORY as the runtime last published it (CC-12), from one read of its snapshot: it starts, writes and remembers
    nothing. C1's fields only, nothing undoable from here (12b), when it's from (`as_of`), whether the runtime runs (its
    owner's `running` and `stale`), and why this answer is a published one (12c)."""
    published = snapshots.read(state, "history")
    if published is None:
        unread = "Kumi's published HISTORY can't be read." if (state / "history.json").exists() else None
        return {"items": [], "from": "last_published", "note": unread or "Kumi hasn't run in this profile yet."}
    rows = published.get("items") if isinstance(published.get("items"), list) else []
    try:
        count = max(1, min(int(limit), snapshots.HISTORY_MAX))  # type: ignore[call-overload]
    except (TypeError, ValueError):
        count = 20
    items = [{**_c1_row(row), "undo_available": False} for row in rows[:count] if isinstance(row, dict)]
    owner = snapshots.owner(state)
    stale, running = owner["stale"], owner.get("running") if isinstance(owner.get("running"), bool) else None
    if scheduled:
        note = "A scheduled job reads HISTORY as Kumi last published it: nothing can be undone from here."
    elif not stale:
        note = ("Kumi runs in another Hermes process: this is HISTORY as it last published it, and nothing can be "
                "undone from here.")
    else:
        note = ("Kumi isn't running: this is HISTORY as Kumi last published it. Nothing can be undone until Kumi "
                "connects again.")
    if stale and running is not False:  # it never said it stopped: it died
        note += " Kumi stopped unexpectedly; its last change may be missing here or unsure."
    return {"items": items, "from": "last_published", "as_of": published.get("at"), "running": running,
            "stale": stale, "note": note}


TRACK_KEYS = {"track", "from_beat", "beats"}
MAX_BEATS = 256
# What a listen that didn't succeed never carries (CC-2's R5): no measurements, no artifact, no MEDIA: line.
MEASURED = ("metrics", "artifact_id", "media_tag", "verdict_text", "capture", "compare", "references",
            "reference_profile", "interpretation", "interpretation_error")


def is_track_listen(args: dict) -> bool:
    """A listen to a track or the mix of the open project (CC-2): it plays the project, so the producer approves it."""
    source = args.get("source")
    return isinstance(source, dict) and "track" in source


def track_bounds(answer: Any, args: dict) -> dict | None:
    """A track listen's bounds (CC-2's A2 and its clarification), as its preview gave them, when each is what the
    runtime may give: `estimated_seconds`, the window (finite, over 0, at most 60); `prepare_seconds`, getting
    `compare_to` ready before anything plays (0 without one, else a whole number up to 120); `deadline_seconds`, the
    native call (a whole number from 1 to 360). None otherwise, and then the listen isn't approved: nothing is guessed.
    The approval keeps them, the handler waits for the two bounds and a margin, and the runtime gets them as `_listen`."""
    if not isinstance(answer, dict):
        return None
    seconds, prepare, deadline = (answer.get(name) for name in ("estimated_seconds", "prepare_seconds",
                                                               "deadline_seconds"))

    def whole(value: Any, most: int) -> bool:
        return isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= most

    if (isinstance(seconds, bool) or not isinstance(seconds, (int, float)) or not math.isfinite(seconds)
            or not 0 < seconds <= runtime.TRACK_SECONDS_MAX):
        return None
    if not whole(prepare, runtime.TRACK_PREPARE_MAX if listen_references(args) else 0):
        return None
    if not whole(deadline, runtime.TRACK_DEADLINE_MAX) or deadline < 1:
        return None
    return {"estimated_seconds": seconds, "prepare_seconds": prepare, "deadline_seconds": deadline}


def listen_refusal(args: dict) -> str | None:
    """Why `kumi_listen` can't take these arguments, or None. A source is a file, a sound or an artifact, or one track
    or the mix over a beat window (CC-2); `compare_to` is never a track. The runtime checks the window against the
    tempo (60 s at most) and the project; this checks only what needs no DAW."""
    source = args.get("source")
    if is_track_listen(args):
        refusal = _track_refusal(source, args)
        if refusal:
            return refusal
    elif not _file_source(source):
        return "source is a file, a sound, an artifact, or a track of the open project."
    if "compare_to" in args and "references" in args:
        return "choose compare_to or references, not both."
    # Runtime.call drops model-supplied private keys; only the host can inject the approved _listen bounds.
    public = {key for key in args if not (isinstance(key, str) and key.startswith("_"))}
    if not public <= {"source", "compare_to", "references", "expected_project_key", "expected_epoch", "preview"}:
        return "listen accepts source and either compare_to or references; capture also needs its project identity."
    if "references" in args and (not isinstance(args["references"], list) or not 1 <= len(args["references"]) <= 5):
        return "references requires 1 to 5 file, sound or artifact references."
    seen = set()
    for reference in listen_references(args):
        if not isinstance(reference, dict) or not _file_source({key: value for key, value in reference.items()
                                                              if key not in ("from", "seconds")}):
            return "a reference is one file, sound with map_revision, or artifact; never a track or mix."
        if set(reference) & {"from", "seconds"}:
            start, seconds = reference.get("from"), reference.get("seconds")
            if (type(start) not in (int, float) or type(seconds) not in (int, float)
                    or not _finite(start) or not _finite(seconds) or start < 0 or not 0 < seconds <= 60
                    or not _finite(start + seconds) or start + seconds <= start):
                return "a reference excerpt needs both finite from >= 0 and 0 < seconds <= 60 with a finite end."
        identity = tuple(sorted(reference.items()))
        if identity in seen:
            return "a reference repeats the same source and window; choose each source/window once."
        seen.add(identity)
    return None


def listen_references(args: dict) -> list:
    return args["references"] if isinstance(args.get("references"), list) else (
        [args["compare_to"]] if "compare_to" in args else [])


def _finite(value: int | float) -> bool:
    try:
        return math.isfinite(value)
    except OverflowError:
        return False


def listen_feature_refusal(host: Host, args: dict, live: runtime.Runtime | None = None) -> str | None:
    """The live handshake alone grants fields. No runtime leaves the existing not_connected path intact."""
    live = live if live is not None else host.runtimes.get(host.data_dir())
    if live is None:
        return None
    features = live.listen_features
    if "references" in args and "references" not in features:
        return "the connected Kumi runtime doesn't support references; use one whole-file compare_to instead."
    if ("window" not in features and any(isinstance(reference, dict) and
            ("from" in reference or "seconds" in reference) for reference in listen_references(args))):
        return "the connected Kumi runtime doesn't support reference excerpts; use a whole-file reference instead."
    return None


def source_kind(source: Any) -> str | None:
    """A listen source's kind, as kumi_release.json's `listen_sources` names them (CC-16)."""
    if not isinstance(source, dict):
        return None
    for key, kind in (("track", "track"), ("file", "file"), ("sound_id", "sound"), ("artifact_id", "artifact")):
        if key in source:
            return kind
    return None


def listen_unsupported(host: Host, args: dict) -> str | None:
    """Why the pinned runtime can't take this listen in the producer's DAW, or None (CC-16: the pin records what each
    DAW's runtime listens to; one that doesn't say listens to files alone). Refused before anything is asked or
    played, in the hook and again here."""
    daw = host.daw()
    kinds = setup.listen_sources(daw, host.plugin_dir)
    name = schemas.DAW_NAMES.get(daw, daw)
    for source in [args.get("source"), *listen_references(args)]:
        kind = source_kind(source)
        if kind is None or kind in kinds:
            continue
        if kind == "track":
            return (f"this version of Kumi can't listen to a track or the mix in {name}. Measure a file instead: "
                    "a bounce of the part, or the sample it plays.")
        return f"this version of Kumi can't measure a {kind} in {name}."
    return None


def _file_source(source: Any) -> bool:
    if not isinstance(source, dict):
        return False
    if set(source) not in ({"file"}, {"artifact_id"}, {"sound_id", "map_revision"}):
        return False
    return all(isinstance(value, str) and value.strip() for value in source.values())


def _track_refusal(source: dict, args: dict) -> str | None:
    if not set(source) <= TRACK_KEYS:
        return "a track source is {track, from_beat, beats} and nothing else."
    track = source.get("track")
    one = (isinstance(track, dict) and set(track) == {"ref"} and isinstance(track["ref"], str)
           and bool(track["ref"].strip()))
    mix = isinstance(track, dict) and set(track) == {"mix"} and track["mix"] in (True, "true")
    if not (one or mix):
        return 'track is {"ref": "<a track ref from kumi_observe>"} or {"mix": true}: one track or the mix.'
    start, beats = _beat(source.get("from_beat")), _beat(source.get("beats"))
    if start is None or start < 0:
        return "from_beat is the beat to start at: 0 or more."
    if beats is None or not 0 < beats <= MAX_BEATS or not math.isfinite(start + beats):
        return f"beats is how many beats to listen to: more than 0 and at most {MAX_BEATS}."
    if not all(isinstance(args.get(key), (str, int)) and not isinstance(args.get(key), bool) and str(args[key]).strip()
               for key in ("expected_project_key", "expected_epoch")):
        return "expected_project_key and expected_epoch are the project key and epoch kumi_observe gave: observe first."
    return None


def _beat(value: Any) -> int | float | None:
    """A finite number of beats, from a number or a number's text (the hook sees the model's arguments uncoerced)."""
    if isinstance(value, str):
        try:
            value = float(value.strip())
        except ValueError:
            return None
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        return None
    return int(value) if float(value).is_integer() else float(value)


def listen_args(args: dict) -> dict:
    """What the runtime's `listen` gets: the model's arguments without `preview` (the approval hook's alone, CC-2), and
    a track's window as numbers, `mix` as true and the epoch as text."""
    sent = {key: value for key, value in args.items() if key != "preview"}
    if is_track_listen(sent):
        source = dict(sent["source"])
        source.update({key: _beat(source[key]) for key in ("from_beat", "beats") if key in source})
        if isinstance(source.get("track"), dict) and "mix" in source["track"]:
            source["track"] = {"mix": True}
        sent["source"] = source
        if "expected_epoch" in sent:
            sent["expected_epoch"] = str(sent["expected_epoch"])
    return sent


def kumi_listen(host: Host, args: dict, **kwargs: Any) -> dict:
    refusal = listen_refusal(args)  # the approval hook checks this too; a dispatch without it mustn't play the DAW
    if refusal:
        return {"error": {"code": "invalid", "reason": refusal}}
    unsupported = listen_unsupported(host, args)
    if unsupported:
        return {"error": {"code": "unsupported", "reason": f"Kumi didn't listen: {unsupported}"}}
    live = host.live()
    unsupported = listen_feature_refusal(host, args, live)
    if unsupported:
        return {"error": {"code": "unsupported", "reason": f"Kumi didn't listen: {unsupported}"}}
    if not is_track_listen(args):
        result = live.call("listen", listen_args(args), host.identity(kwargs), timeout=WRITE_TIMEOUT)
        return interpreted(host, kept_media(host, measured_result(result)))
    # A track listen plays the project: sent once, and an answer that never came is unsure, never resent (CC-2).
    call = kwargs.get("kumi_call")
    bounds = track_bounds(call.extra.get("listen"), args) if isinstance(call, calls.Entry) else None
    if bounds is None:  # approved_only let it through, so the hook approved it with its bounds; never guess them
        return {"error": {"code": "not_approved", "reason": "Kumi didn't play anything: this listen came without "
                                                            "its runtime's deadline."}}
    timeout = bounds["prepare_seconds"] + bounds["deadline_seconds"] + runtime.TRACK_MARGIN
    try:
        result = live.call("listen", listen_args(args), host.identity(kwargs), timeout=timeout,
                                  interrupted=host.stop_watch(), private={"_listen": bounds})
    except runtime.Unavailable as failure:
        if failure.code not in UNSURE_CODES:
            raise
        return {"state": "unsure", "code": failure.code, "reason": failure.reason, "cleanup": "unsure",
                "hint": UNSURE_HINT}
    result = measured_result(result)
    return interpreted(host, kept_media(host, result))


def measured_result(result: dict) -> dict:
    """A failed measurement cannot carry reference/profile facts or media, even from a malformed runtime reply."""
    if "error" in result or result.get("state") in ("refused", "cancelled", "unsure"):
        result = {key: value for key, value in result.items() if key not in MEASURED}
        if result.get("state") == "unsure":
            result.setdefault("hint", UNSURE_HINT)
    return result


def kept_media(host: Host, result: dict) -> dict:
    """A result's `media_tag` (`MEDIA:<path>`) passes only when it names the very file its artifact is kept as in the
    profile's artifacts (`snapshots.artifact_file`), and then names that file by its real path. Hermes delivers a
    MEDIA: path through links, with only a denylist, so a crafted one could hand a chat a file from elsewhere
    (research-1's review, item 16; rgzm). The measurements stay either way."""
    # Native currently supplies playback only for the source. A nested tag needs the same retained-file proof.
    if isinstance(result.get("compare"), dict):
        result = {**result, "compare": kept_media(host, result["compare"])}
    if isinstance(result.get("references"), list):
        result = {**result, "references": [kept_media(host, item) if isinstance(item, dict) else item
                                           for item in result["references"]]}
    tag = result.get("media_tag")
    if tag is None:
        return result
    artifact = result.get("artifact_id")
    kept = snapshots.artifact_file(Path(host.data_dir()) / "state", artifact) if isinstance(artifact, str) else None
    named = snapshots.local(tag[len("MEDIA:"):]) if isinstance(tag, str) and tag.startswith("MEDIA:") else None
    if kept is not None and named is not None and snapshots.resolved(named) == kept:
        return {**result, "media_tag": f"MEDIA:{kept}"}
    log.warning("Kumi left out a listen's MEDIA: line that doesn't name its kept file")
    return {key: value for key, value in result.items() if key != "media_tag"}


def interpreted(host: Host, result: dict) -> dict:
    """CC-1: with the producer's `listen_interpretation` on, a measured result gets the reading of Hermes's listening
    model (`interpretation`: its numbers and a band-energy image, never the audio), or why there's none
    (`interpretation_error`). The measurements stay either way, and nothing is asked again of the runtime or the DAW.
    A listen that didn't measure (refused, cancelled, unsure, or an error) is never sent to a model."""
    if (not host.listen_interpretation() or "error" in result or not isinstance(result.get("metrics"), dict)
            or result.get("state") not in (None, "done")):
        return result
    try:
        return {**result, "interpretation": listening.interpret(host.llm(), result,
                                                               timeout=host.interpretation_timeout())}
    except listening.InterpretationError as failure:
        return {**result, "interpretation_error": str(failure)}
    except Exception:
        log.exception("Kumi's listening interpretation failed")
        return {**result, "interpretation_error": "Model interpretation failed; the measurements remain available."}


def kumi_sounds(host: Host, args: dict, **kwargs: Any) -> dict:
    """The runtime's sound search, as the model may see it: sound ids, what the map says of each and why each matched
    (CC-23), never a file's path (CC-4: a path is the media route's alone)."""
    result = host.live().call("sounds", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    items = result.get("items")
    if isinstance(items, list):
        result["items"] = [_sound(item) if isinstance(item, dict) else item for item in items]
    return result


# A reason that names a file or folder: an absolute or home path, a backslash, a drive, or two slashes in a word. Serve
# keeps paths out of its reasons (CC-23's 23b and 23-r1); this is the plugin's second guard.
_PATHLIKE = re.compile(r"(?:^|\s)~?/|\\|\b[A-Za-z]:/|\S/\S*/")


def _sound(item: dict) -> dict:
    sound = {key: value for key, value in item.items() if key != "path"}
    if "why" in sound:
        why = sound["why"] if isinstance(sound["why"], list) else []
        sound["why"] = [reason for reason in why if isinstance(reason, str) and not _PATHLIKE.search(reason)]
    return sound


# The palette (palettes.py): reads pass like kumi_sounds; edits change no project, so they're never asked, but like
# every write they never run with nobody in the chat (policy.py).
PALETTE_READS = frozenset({"list", "search", "neighbours", "kit"})
PALETTE_LISTED = 50
# A kit's most sounds (Live's Drum Rack has 128 pads), and how many palette sounds a neighbours read asks about.
KIT_MOST = 128
NEIGHBOUR_SEEDS = 20
PALETTE_NONE = ("The producer has no palette yet: gathering sounds on the map in Desktop, or kumi_palette add, makes "
                "one; so does kumi_palette new with a name.")
PALETTE_OFF_MAP = ("Those sound ids aren't on the current map (ids belong to one map revision): find them again with "
                   "kumi_sounds or kumi_palette list.")
PALETTE_STRICT = ("The palette is strict: it brings nothing in. kumi_palette loose makes it loose, when the producer "
                  "agrees.")
PALETTE_NOTHING_HERE = "None of the palette's sounds is on this sound map, so there's nothing of it to use here."
KIT_SAY = {
    "ableton": ("To load them as a kit in Live: look (kumi_observe, scope focus) for the Drum Rack on the selected "
                "track; with none, say so and offer to load one first. Then one kumi_change per sound, in this order, "
                "each alone in its change: load_sample_to_pad with the rack's deviceRef, the sound as sample "
                "{sound_id, map_revision}, on the next empty pad from note 36 (C1) up. Never clear a pad to make room. "
                "Kumi can't undo a pad load: Live's own Undo takes each one back, one step at a time, right away."),
    "renoise": ("To load them as a kit in Renoise: sample_load each into the selected instrument as a new sample "
                "(the sound as {sound_id, map_revision}), then keyzone_set each to its own key (noteRange and "
                "baseNote), consecutive from C-3 (note 36) up, in this order."),
    "reaper": ("REAPER has no drum pads Kumi can load: say so, and offer to place them as items on a track "
               "(item_import)."),
}


def kumi_palette(host: Host, args: dict, **kwargs: Any) -> dict:
    """The producer's palette, shared with the map UI (palettes.py). Sound ids in and out are the current map
    revision's; no path ever leaves. Every edit tells Desktop (`palette.changed`)."""
    state = Path(host.data_dir()) / "state"
    action = args.get("action")
    the_map = palettes.current_map(state, snapshots.map_current(state),
                                   lambda revision: snapshots.map_revision(state, revision))
    try:
        if action in PALETTE_READS:
            return palette_read(host, state, the_map, action, args, kwargs)
        result = palette_edit(host, state, the_map, action, args)
    except palettes.NoPalette:
        return palette_none(state)
    except palettes.OffMap:
        return {"error": {"code": "not_on_map", "reason": PALETTE_OFF_MAP}}
    except palettes.Invalid as invalid:
        return {"error": {"code": "invalid", "reason": str(invalid)}}
    except palettes.Busy as busy:
        return {"error": {"code": "busy", "reason": str(busy)}}
    try:
        broadcast("palette.changed", {"seq": palettes.seq(state), "profile": host.profile()})
    except Exception:
        log.exception("Kumi couldn't tell Desktop its palette changed")
    return result


def palette_none(state: Path) -> dict:
    """Native's "no palette" answer, with the palettes there are to `use`."""
    others = palettes.listed(state)
    return {"palette": None, "say": PALETTE_NONE, **({"palettes": others} if others else {})}


def _palette_ids(args: dict, field: str, most: int, action: str) -> list[str]:
    value = args.get(field)
    if value is None:
        return []
    if not isinstance(value, list) or not 1 <= len(value) <= most or any(not isinstance(v, str) for v in value):
        raise palettes.Invalid(f"{action}'s {field} is a list of 1 to {most} ids.")
    return value


def palette_edit(host: Host, state: Path, the_map: Any, action: Any, args: dict) -> dict:
    if action == "add":
        sound_ids = _palette_ids(args, "sound_ids", 200, "add")
        artifact_ids = _palette_ids(args, "artifact_ids", palettes.MOST_ARTIFACTS, "add")
        if not sound_ids and not artifact_ids:
            raise palettes.Invalid("add takes sound_ids (from kumi_sounds or list) or artifact_ids (Kumi results).")
        folders = host.library_folders() if artifact_ids else []
        artifacts = [(each, snapshots.palette_artifact(state, each, folders)) for each in artifact_ids]
        if any(name is None for _, name in artifacts):
            raise palettes.Invalid("Kumi can't play one of those results here, so it can't go in the palette: "
                                   "artifact_ids are Kumi results' artifact_id.")
        return {"said": palettes.gather(state, the_map, sound_ids, artifacts)}
    if action == "remove":
        sound_ids = _palette_ids(args, "sound_ids", 200, "remove")
        keys = _palette_ids(args, "keys", 200, "remove")
        if bool(sound_ids) == bool(keys):
            raise palettes.Invalid("remove takes sound_ids or keys (as list gives them), one of the two.")
        if keys:
            return {"said": palettes.remove_keys(state, keys)}
        return {"removed": palettes.remove_sounds(state, the_map, sound_ids)}
    if action == "exclude":
        sound_ids = _palette_ids(args, "sound_ids", 200, "exclude")
        if not sound_ids:
            raise palettes.Invalid("exclude takes sound_ids: the sounds the palette must never bring in.")
        return {"excluded": palettes.exclude_sounds(state, the_map, sound_ids)}
    if action in ("new", "use", "rename", "forget"):
        if not isinstance(args.get("name"), str) or not args["name"].strip():
            raise palettes.Invalid(f"{action} takes name: a palette's name.")
        edit = {"new": palettes.new, "use": palettes.use, "rename": palettes.rename, "forget": palettes.forget}
        return {"said": edit[action](state, args["name"])}
    if action == "strict":
        return {"said": palettes.reach(state, 0)}
    if action == "loose":
        reach = args.get("reach", palettes.LOOSE)
        if not isinstance(reach, int) or isinstance(reach, bool) or not 1 <= reach <= palettes.FARTHEST:
            raise palettes.Invalid(f"loose takes reach: how many of each sound's nearest to bring, from 1 to "
                                   f"{palettes.FARTHEST}.")
        return {"said": palettes.reach(state, reach)}
    if action == "move":
        key, to = args.get("key"), args.get("to")
        if not isinstance(key, str) or not isinstance(to, int) or isinstance(to, bool) or to < 0:
            raise palettes.Invalid("move takes key (as list gives it) and to (its new place, 0 first).")
        return {"said": palettes.move(state, key, to)}
    raise palettes.Invalid(f"action is one of {', '.join(schemas.PALETTE_ACTIONS)}.")


def _described(item: dict) -> dict:
    """A palette item for the model: its key, its sound id on this map (or null) or its artifact id, name and class."""
    if item["source"] == "artifact":
        return {"key": item["key"], "sound_id": None, "artifact_id": item["artifact_id"], "name": item["name"]}
    return {"key": item["key"], "sound_id": item["sound_id"], "name": item["name"],
            **({"class": item["class"]} if item["class"] else {})}


def palette_read(host: Host, state: Path, the_map: Any, action: str, args: dict, kwargs: dict) -> dict:
    palette = palettes.current(state, the_map)
    if palette is None:
        return palette_none(state)
    limit = args.get("limit", PALETTE_LISTED)
    limit = min(max(limit, 1), 200) if isinstance(limit, int) and not isinstance(limit, bool) else PALETTE_LISTED
    items = palette["items"]
    out = {"palette": {"name": palette["name"], "mode": palette["mode"], "items": len(items)},
           "map_revision": the_map.revision if the_map else None}
    if action == "list":
        out["sounds"] = [_described(item) for item in items[:limit]]
        if len(items) > limit:
            out["more"] = len(items) - limit
        if palette["excluded"]:
            out["excluded"] = [_described(item) for item in palette["excluded"][:limit]]
        out["palettes"] = palette["palettes"]
        return out
    if action == "search":
        words = str(args.get("words") or "").lower().split()
        if not words:
            raise palettes.Invalid("search takes words: what its sounds have in their name, class or folder.")
        found = [item for item in items
                 if all(word in " ".join(filter(None, (item["name"], item["class"], item["folder"]))).lower()
                        for word in words)]
        out["sounds"] = [_described(item) for item in found[:limit]]
        if len(found) > limit:
            out["more"] = len(found) - limit
        return out
    on_map = [item for item in items if item["sound_id"]]
    if action == "kit":
        kit = on_map[:KIT_MOST]
        out["sounds"] = [{"sound_id": item["sound_id"], "name": item["name"],
                          **({"class": item["class"]} if item["class"] else {})} for item in kit]
        out["left_out"] = len(items) - len(kit)
        out["say"] = KIT_SAY[host.daw()] if kit else PALETTE_NOTHING_HERE
        return out
    if palette["reach"] == 0:
        return {**out, "say": PALETTE_STRICT}
    if not on_map:
        return {**out, "say": PALETTE_NOTHING_HERE}
    return {**out, **palette_neighbours(host, palette, on_map, limit, kwargs)}


def palette_neighbours(host: Host, palette: dict, on_map: list[dict], limit: int, kwargs: dict) -> dict:
    """What a loose palette brings in: for each of its first 20 sounds on this map, its `reach` nearest by serve's
    `sounds` near search (none of the palette's own or excluded), the ones near more of its sounds first, then the
    closest."""
    reach = palette["reach"]
    own = {item["sound_id"] for item in palette["items"] + palette["excluded"] if item["sound_id"]}
    live = host.live()
    found: dict[str, dict] = {}
    for seed in on_map[:NEIGHBOUR_SEEDS]:
        answer = live.call("sounds", {"query": "", "near": seed["sound_id"], "limit": 2 * reach},
                           host.identity(kwargs), timeout=READ_TIMEOUT)
        hits = [hit for hit in answer.get("items") or []
                if isinstance(hit, dict) and isinstance(hit.get("sound_id"), str) and hit["sound_id"] not in own]
        for hit in hits[:reach]:
            closeness = hit.get("closeness") if isinstance(hit.get("closeness"), int) else 0
            entry = found.setdefault(hit["sound_id"], {"sound_id": hit["sound_id"], "name": hit.get("name"),
                                                       "class": hit.get("class"), "near": [], "closeness": closeness})
            if seed["name"] not in entry["near"]:
                entry["near"].append(seed["name"])
            entry["closeness"] = max(entry["closeness"], closeness)
    ranked = sorted(found.values(), key=lambda each: (-len(each["near"]), -each["closeness"], str(each["name"])))
    sounds = [{"sound_id": each["sound_id"], "name": each["name"],
               **({"class": each["class"]} if isinstance(each["class"], str) else {}), "near": each["near"]}
              for each in ranked[:limit]]
    return {"sounds": sounds, **({"more": len(ranked) - limit} if len(ranked) > limit else {})}


LIBRARY_ACTIONS = ("status", "add_folder", "scan", "cancel")


def library_refusal(args: dict) -> str | None:
    """What's wrong with a kumi_library call's arguments, or None; the hook checks this, and the handler again, before
    anything reaches the runtime (CC-14): the action, a cancel's job id in the runtime's job-id form, a folder never
    with cancel and a job id only with it."""
    action = args.get("action")
    if action not in LIBRARY_ACTIONS:
        return "kumi_library's action is status, add_folder, scan or cancel."
    if action != "cancel":
        return "job_id goes only with cancel." if args.get("job_id") is not None else None
    if args.get("folder") is not None:
        return "cancel takes the scan's job_id, never a folder."
    if snapshots.file_id(args.get("job_id")) is None:
        return "cancel needs the scan's job_id, as the scan returned it."
    return None


def kumi_library(host: Host, args: dict, **kwargs: Any) -> dict:
    """The library tool; a folder the producer approved adding is also kept in the plugin's library_folders setting, so
    the next runtime starts with it (`--library-root`). A cancel asks the runtime to stop the scan it names (CC-14): the
    reply acknowledges the request, and kumi_job says `cancelled` once the scan has stopped."""
    refusal = library_refusal(args)
    if refusal is not None:
        return {"error": {"code": "invalid", "reason": refusal}}
    action = args["action"]
    if action == "cancel":
        args = {"action": "cancel", "job_id": args["job_id"]}
    if action == "add_folder":
        folder, refusal = snapshots.library_folder(args.get("folder"), hermes_home=host.hermes_home())
        if refusal is not None:
            return {"error": {"code": "invalid", "reason": refusal}}
        args = {**args, "folder": folder}
    result = host.live().call("library", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    if action == "add_folder" and "error" not in result:
        folders = host.library_folders()
        if args["folder"] not in folders:
            host.set_library_folders([*folders, args["folder"]])
    return result


def kumi_generate(host: Host, args: dict, **kwargs: Any) -> dict:
    """Paid generation with Mirelo (CC-13): a quote, then the create the producer approves from that quote. The
    generation itself is a job, followed with kumi_job."""
    refusal = generate.refusal(args)
    if refusal is not None:
        return {"error": {"code": "invalid_request", "reason": refusal}}
    identity = host.identity(kwargs)
    if args["action"] == "quote":
        return generate.quote(host, args, identity, kwargs)
    return generate.create(host, args, identity, kwargs)


def kumi_job(host: Host, args: dict, **kwargs: Any) -> dict:
    """A long job by its id: a generation's from its runtime's `generate`, settled in the credit ledger (CC-13), its
    MEDIA: line only for its kept file; any other job from the runtime's `job`. A generation the ledger never bound
    (its create's answer was lost) is recognized in that plain read and adopted by its quote (generate.adopt)."""
    identity = host.identity(kwargs)
    job_id = args.get("job_id")
    routed = generate.find(host, job_id)
    if routed is None:
        plain = host.live().call("job", args, identity, timeout=READ_TIMEOUT)
        routed = generate.adopt(host, job_id, plain)
        if routed is None:
            return plain
    found = generate.follow(host, *routed, job_id, identity, READ_TIMEOUT)
    result = found.get("result")
    if isinstance(result, dict) and "media_tag" in result:
        found = {**found, "result": kept_media(host, result)}
    return found


# Where play can start, per DAW (CC-17, its W5-a: a static table that refuses, never over-offers; Live's from_bar and
# from_beat join it in the release that ships #1074's start-time seam, with its re-pin).
TRANSPORT_FIELDS = {"ableton": frozenset({"clip_ref", "scene_index"}), "reaper": frozenset({"from_bar", "from_beat"}),
                    "renoise": frozenset({"from_slot", "from_line"})}
_TRANSPORT_WHERE = {"ableton": ("Live plays the Arrangement from its own start marker here: launch a Session clip "
                                "(clip_ref) or a scene (scene_index) instead"),
                    "reaper": "REAPER plays from a bar (from_bar) or a beat (from_beat)",
                    "renoise": "Renoise plays from a sequence slot (from_slot) and its line (from_line)"}


def transport_refusal(host: Host, args: dict) -> str | None:
    """Why kumi_transport can't take these arguments in the producer's DAW, or None; the hook checks this, and the
    handler again. stop takes nothing else; play takes at most one place to start (Renoise's slot and line are one)."""
    action = args.get("action")
    given = set(args) - {"action", "expected_project_key"}
    if action == "stop":
        return "stop takes nothing else." if given or "expected_project_key" in args else None
    if action != "play":
        return "action is play or stop."
    daw = host.daw()
    unknown = sorted(given - TRANSPORT_FIELDS.get(daw, frozenset()))
    if unknown:
        return f"{', '.join(unknown)} isn't how play starts here: {_TRANSPORT_WHERE.get(daw, 'play from the playhead')}."
    if len(given - {"from_line"}) > 1:
        return "play starts from one place."
    if not isinstance(args.get("expected_project_key"), str) or not args["expected_project_key"]:
        return "play takes kumi_observe's project_key, as expected_project_key."
    return None


def kumi_transport(host: Host, args: dict, **kwargs: Any) -> dict:
    """Start or stop playback (CC-17): a playback-class action the producer approves (play only), with no HISTORY."""
    refusal = transport_refusal(host, args)
    if refusal is not None:
        return {"error": {"code": "invalid", "reason": refusal}}
    return host.live().call("transport", {key: value for key, value in args.items() if key != "preview"},
                            host.identity(kwargs), timeout=READ_TIMEOUT)



# name, schema, handler, emoji: every one registered every time (catalog rule 6).
TOOLS: tuple[tuple[str, dict, Callable[..., dict], str], ...] = (
    ("kumi_status", schemas.KUMI_STATUS, kumi_status, "🎛️"),
    ("kumi_setup", schemas.KUMI_SETUP, kumi_setup, "🧰"),
    ("kumi_connect", schemas.KUMI_CONNECT, kumi_connect, "🔌"),
    ("kumi_observe", schemas.KUMI_OBSERVE, _forward("observe", READ_TIMEOUT), "👀"),
    ("kumi_read", schemas.KUMI_READ, _forward("read", READ_TIMEOUT), "📖"),
    ("kumi_plugin_knowledge", schemas.KUMI_PLUGIN_KNOWLEDGE, kumi_plugin_knowledge, ""),
    ("kumi_change", schemas.KUMI_CHANGE, kumi_change, "🎚️"),
    ("kumi_undo", schemas.KUMI_UNDO, kumi_undo, "↩️"),
    ("kumi_history", schemas.KUMI_HISTORY, kumi_history, "🕘"),
    ("kumi_listen", schemas.KUMI_LISTEN, kumi_listen, "📈"),
    ("kumi_sounds", schemas.KUMI_SOUNDS, kumi_sounds, "🔎"),
    ("kumi_palette", schemas.KUMI_PALETTE, kumi_palette, "🎨"),
    ("kumi_library", schemas.KUMI_LIBRARY, kumi_library, "🗂️"),
    ("kumi_job", schemas.KUMI_JOB, kumi_job, "⏳"),
    ("kumi_generate", schemas.KUMI_GENERATE, kumi_generate, "🪄"),
    ("kumi_transport", schemas.KUMI_TRANSPORT, kumi_transport, "▶️"),
    ("kumi_script", schemas.KUMI_SCRIPT, kumi_script, "📜"),
    ("kumi_variations", schemas.KUMI_VARIATIONS, kumi_variations, "🔀"),
    ("kumi_device", schemas.KUMI_DEVICE, kumi_device, "🧩"),
    ("kumi_preset_build", schemas.KUMI_PRESET_BUILD, kumi_preset_build, "🎚️"),
    ("kumi_preset", schemas.KUMI_PRESET, kumi_preset, "🎛️"),
    ("kumi_judge", schemas.KUMI_JUDGE, kumi_judge, "⚖️"),
    ("kumi_tune", schemas.KUMI_TUNE, kumi_tune, "🎯"),
    ("kumi_wavetable", schemas.KUMI_WAVETABLE, kumi_wavetable, "🌊"),
    ("kumi_design", schemas.KUMI_DESIGN, kumi_design, "🧪"),
    ("kumi_sound", schemas.KUMI_SOUND, kumi_sound, "🔬"),
    ("kumi_audition", schemas.KUMI_AUDITION, kumi_audition, "🎧"),
    ("kumi_groove", schemas.KUMI_GROOVE, kumi_groove, "🥁"),
    ("kumi_render", schemas.KUMI_RENDER, kumi_render, "💿"),
)
# What a tool needs before it's shown to the model: kumi_status, kumi_setup and kumi_connect always are (RESEARCH.md
# §5.4).
ALWAYS_SHOWN = {"kumi_status", "kumi_setup", "kumi_connect"}


def approved_only(name: str, args: dict) -> bool:
    """The calls policy.py clears, asking the producer or (for a change, by default) not: they run only as cleared,
    with what the hook's preview bound (CC-5)."""
    return (name in ("kumi_change", "kumi_undo", "kumi_script", "kumi_device", "kumi_preset", "kumi_judge",
                     "kumi_tune", "kumi_wavetable", "kumi_sound", "kumi_audition", "kumi_render")
            or (name == "kumi_groove" and groove_changes(args))
            or (name == "kumi_variations" and args.get("action") in ("make", "change", "keep"))
            or (name == "kumi_library" and args.get("action") in ("add_folder", "scan"))
            or (name == "kumi_listen" and is_track_listen(args))
            or (name == "kumi_setup" and args.get("action") in ("install", "adopt"))
            or (name == "kumi_generate" and (args.get("action") == "create"
                                             or (args.get("action") == "quote" and args.get("kind") == "audio_to_midi")))
            or (name == "kumi_transport" and args.get("action") == "play"))


def handler(name: str, function: Callable[..., dict], host: Host) -> Callable[..., str]:
    """`function` as Hermes calls tool `name`: `(args, **context) -> str`, never raising. The approval hook's record of
    the call (CC-5) gives it the model's tool-call id; a call the producer approves runs only with the arguments the
    approval saw, and never without it. In a scheduled job, only the published reads run, whatever reached here."""

    def call(args: Any = None, **kwargs: Any) -> str:
        args = args if isinstance(args, dict) else {}
        record = host.calls.take(name, args, kwargs)
        try:
            if name not in SCHEDULED_READS and host.unattended() == "scheduled":
                result = {"error": {"code": "scheduled", "reason": SCHEDULED}}
            elif approved_only(name, args) and not (record and record.extra.get("approved")):
                result = {"error": {"code": "not_approved", "reason": (
                    "Kumi didn't run this: it isn't the call the producer approved (its arguments changed after the "
                    "approval, or it came without Hermes's approval step).")}}
            else:
                result = function(host, args, **{**kwargs, "kumi_call": record})
        except runtime.Refused as refused:
            result = refused.as_json()
        except runtime.Unavailable as failure:
            result = {"error": {"code": failure.code, "reason": failure.reason}}
        except Exception:
            log.exception("Kumi's %s failed", name)
            result = {"error": {"code": "failed", "reason": "Kumi's plugin failed; Hermes's log says why."}}
        return json.dumps(result, ensure_ascii=False)

    call.__name__ = name
    return call


def installed_check(host: Host) -> Callable[[], bool]:
    """A tool's `check_fn`: shown once Kumi is set up in this profile (it reads one small file; no runtime call)."""

    def check() -> bool:
        try:
            host.install(host.data_dir())
        except runtime.Unavailable:
            return False
        except Exception:
            log.exception("Kumi couldn't tell whether it's set up")
            return False
        return True

    return check
