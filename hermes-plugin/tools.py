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

from . import calls, devices, generate, listening, runtime, schemas, scripts, setup, snapshots

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
        if not isinstance(answer, dict) or answer.get("state") not in ("applied", "partial", "ran", "failed"):
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
        """Keep a chat's write into the User Library (CC-22's device build) as its newest step. Kumi never undoes one:
        the file stays, as its approval said. So "undo that" right after it undoes nothing, never the change before it
        (CC-26's 26-w5)."""
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
    approvals note, and the runtime's own status."""
    daw = host.daw()
    data_dir = host.data_dir()
    report: dict[str, Any] = {"setup": setup.for_model(setup.status(daw=daw, data_dir=data_dir,
                                                                    plugin_dir=host.plugin_dir)),
                              "runtime": None, "connected": False, "daw": daw, "project": None, "owner": None,
                              "capabilities": []}
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


def _write(host: Host, tool: str, args: dict, kwargs: dict) -> dict:
    """A write (C2): sent once. An answer that never came, or can't be read, means the outcome is unknown: reported as
    unsure, never resent, and never called unchanged. The producer's Stop cancels it while it waits (`notifications/
    cancelled`), so its remaining steps don't go on applying after Hermes has given the call up (research-1's review,
    item 7); that's unsure too."""
    live = host.live()
    try:
        answer = live.call(tool, args, host.identity(kwargs), timeout=WRITE_TIMEOUT, interrupted=host.stop_watch())
    except runtime.Unavailable as failure:
        if failure.code not in UNSURE_CODES:
            raise
        return {"state": "unsure", "code": failure.code, "reason": failure.reason, "hint": UNSURE_HINT}
    if tool != "undo":  # every other write that made rows is the chat's newest change (CC-3 A2)
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


def kumi_change(host: Host, args: dict, **kwargs: Any) -> dict:
    change = {key: value for key, value in args.items() if key != "preview"}  # previews are the approval hook's (CC-3)
    if "expected_epoch" in change:
        change["expected_epoch"] = str(change["expected_epoch"])
    answer = _write(host, "change", change, kwargs)
    note = copy_note(answer.get("copy")) if isinstance(answer, dict) else None
    return {**answer, "copy_note": note} if note else answer


# The copy of the Set a Live plan keeps beside it before its third step or a step that could lose material, as serve's
# change answer gives it (`copy`: the file's name and why, or why it couldn't be kept). Told to the model as standalone
# Kumi tells it, by the file's name: serve keeps the producer's paths out of its answers.
COPY_WHY = {"third_step": "before the plan's third step", "deletes": "before a step that could lose material"}


def copy_note(copy: Any) -> str | None:
    """What the model tells the producer about the copy of their Set this change kept, or couldn't keep."""
    if not isinstance(copy, dict):
        return None
    file, why = copy.get("file"), COPY_WHY.get(copy.get("why"))
    if isinstance(file, str) and file.strip() and not {"/", "\\"} & set(file):
        return (f"Before this, Kumi kept a copy of the producer's Set as last saved, next to it: {_words(file, 120)}"
                f"{f' ({why})' if why else ''}. Tell the producer in a few words, with the file's name.")
    failed = copy.get("failed")
    if isinstance(failed, str) and failed.strip():
        return f"Kumi couldn't keep a copy of the producer's Set before this ({_words(failed, 200)}). Tell the producer."
    return None


def _words(text: str, limit: int) -> str:
    """`text` as one line of at most `limit` characters, without control or invisible format characters."""
    line = " ".join("".join(char for char in text if char.isprintable()).split())
    return line if len(line) <= limit else line[:limit - 1].rstrip() + "…"


# kumi_undo with no id, where this chat made no change Kumi remembers (CC-3 amendment A2), or where its newest step
# wrote a file into the User Library, which Kumi never undoes (26-w5); and how many of a chat's writes Kumi keeps for it.
NOTHING_TO_UNDO = ("Kumi didn't undo anything: this chat made no change Kumi remembers since Hermes started, or it "
                   "undid them all. kumi_history lists Kumi's changes; undo one by its id.")
FILE_STAYS = ("Kumi didn't undo anything: this chat's newest step wrote {file} into your User Library, which Kumi "
              "never undoes (the file stays, as its approval said). kumi_history lists Kumi's changes; undo one by its "
              "id.")
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
    """The copy of the Set a change kept, as a HISTORY row may say it outside Kumi: the file's name (never a path, which
    serve doesn't send) and why, or only that it couldn't be kept: its reason is runtime text, which can name a path
    (12f), so it stays in the change's own result."""
    if not isinstance(copy, dict):
        return None
    why = copy.get("why") if copy.get("why") in COPY_WHY else None
    file = copy.get("file")
    if isinstance(file, str) and file.strip() and not {"/", "\\"} & set(file):
        return {"file": _words(file, 120), "why": why}
    return {"file": None, "why": why, "failed": True} if copy.get("failed") else None


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
    ("kumi_library", schemas.KUMI_LIBRARY, kumi_library, "🗂️"),
    ("kumi_job", schemas.KUMI_JOB, kumi_job, "⏳"),
    ("kumi_generate", schemas.KUMI_GENERATE, kumi_generate, "🪄"),
    ("kumi_transport", schemas.KUMI_TRANSPORT, kumi_transport, "▶️"),
    ("kumi_script", schemas.KUMI_SCRIPT, kumi_script, "📜"),
    ("kumi_variations", schemas.KUMI_VARIATIONS, kumi_variations, "🔀"),
    ("kumi_device", schemas.KUMI_DEVICE, kumi_device, "🧩"),
)
# What a tool needs before it's shown to the model: kumi_status, kumi_setup and kumi_connect always are (RESEARCH.md
# §5.4).
ALWAYS_SHOWN = {"kumi_status", "kumi_setup", "kumi_connect"}


def approved_only(name: str, args: dict) -> bool:
    """The calls policy.py puts to the producer: they run only as approved (CC-5)."""
    return (name in ("kumi_change", "kumi_undo", "kumi_script", "kumi_device")
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
