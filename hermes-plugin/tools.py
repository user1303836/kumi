"""Kumi's Hermes tools (RESEARCH.md C1): each handler takes the model's arguments and Hermes's call context, and returns
JSON. A handler never raises into Hermes: what goes wrong comes back as `{"error": {"code", "reason"}}`.
"""
from __future__ import annotations

import json
import logging
import math
import os
from pathlib import Path
import threading
import time
from typing import Any, Callable
import uuid

from . import calls, generate, listening, runtime, schemas, setup, snapshots

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
        ids = [item["sound_id"] for item in data.get("items") or []
               if isinstance(item, dict) and isinstance(item.get("sound_id"), str)][:SOUNDS_FOUND]
        if not ids:
            return
        event, payload = "sounds.found", {**base, "map_revision": data.get("map_revision"), "sound_ids": ids}
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


def _write(host: Host, tool: str, args: dict, kwargs: dict) -> dict:
    """A write (C2): sent once. An answer that never came, or can't be read, means the outcome is unknown: reported as
    unsure, never resent, and never called unchanged. The producer's Stop cancels it while it waits (`notifications/
    cancelled`), so its remaining steps don't go on applying after Hermes has given the call up (research-1's review,
    item 7); that's unsure too."""
    live = host.live()
    try:
        return live.call(tool, args, host.identity(kwargs), timeout=WRITE_TIMEOUT, interrupted=host.stop_watch())
    except runtime.Unavailable as failure:
        if failure.code not in UNSURE_CODES:
            raise
        return {"state": "unsure", "code": failure.code, "reason": failure.reason, "hint": UNSURE_HINT}


def kumi_change(host: Host, args: dict, **kwargs: Any) -> dict:
    change = {key: value for key, value in args.items() if key != "preview"}  # previews are the approval hook's (CC-3)
    if "expected_epoch" in change:
        change["expected_epoch"] = str(change["expected_epoch"])
    return _write(host, "change", change, kwargs)


def kumi_undo(host: Host, args: dict, **kwargs: Any) -> dict:
    return _write(host, "undo", {key: value for key, value in args.items() if key != "preview"}, kwargs)


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
    return {key: row[key] for key in HISTORY_FIELDS if key in row}


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


SOURCE_KEYS = {"file", "sound_id", "map_revision", "artifact_id"}
TRACK_KEYS = {"track", "from_beat", "beats"}
MAX_BEATS = 256
# What a listen that didn't succeed never carries (CC-2's R5): no measurements, no artifact, no MEDIA: line.
MEASURED = ("metrics", "artifact_id", "media_tag", "verdict_text", "capture")


def is_track_listen(args: dict) -> bool:
    """A listen to a track or the mix of the open project (CC-2): it plays the project, so the producer approves it."""
    source = args.get("source")
    return isinstance(source, dict) and "track" in source


def track_deadline(value: Any) -> int | None:
    """A track listen preview's `deadline_seconds` (CC-2's A2: the runtime's whole bound for the real call) when it's a
    whole number of seconds from 1 to 360; None otherwise, and then the listen isn't approved."""
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= runtime.TRACK_DEADLINE_MAX:
        return None
    return value


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
    if args.get("compare_to") is not None and not _file_source(args["compare_to"]):
        return "compare_to is a file, a sound or an artifact."
    return None


def _file_source(source: Any) -> bool:
    return isinstance(source, dict) and bool(source) and set(source) <= SOURCE_KEYS


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
    if not is_track_listen(args):
        result = host.live().call("listen", listen_args(args), host.identity(kwargs), timeout=WRITE_TIMEOUT)
        return interpreted(host, kept_media(host, result))
    # A track listen plays the project: sent once, and an answer that never came is unsure, never resent (CC-2).
    call = kwargs.get("kumi_call")
    deadline = track_deadline(call.extra.get("deadline")) if isinstance(call, calls.Entry) else None
    if deadline is None:  # approved_only let it through, so the hook approved it with its deadline; never guess one
        return {"error": {"code": "not_approved", "reason": "Kumi didn't play anything: this listen came without "
                                                            "its runtime's deadline."}}
    timeout = deadline + runtime.TRACK_MARGIN
    try:
        result = host.live().call("listen", listen_args(args), host.identity(kwargs), timeout=timeout,
                                  interrupted=host.stop_watch())
    except runtime.Unavailable as failure:
        if failure.code not in UNSURE_CODES:
            raise
        return {"state": "unsure", "code": failure.code, "reason": failure.reason, "cleanup": "unsure",
                "hint": UNSURE_HINT}
    if result.get("state") in ("refused", "cancelled", "unsure"):
        result = {key: value for key, value in result.items() if key not in MEASURED}
        if result["state"] == "unsure":
            result.setdefault("hint", UNSURE_HINT)
        return result
    return interpreted(host, kept_media(host, result))


def kept_media(host: Host, result: dict) -> dict:
    """A result's `media_tag` (`MEDIA:<path>`) passes only when it names the very file its artifact is kept as in the
    profile's artifacts (`snapshots.artifact_file`), and then names that file by its real path. Hermes delivers a
    MEDIA: path through links, with only a denylist, so a crafted one could hand a chat a file from elsewhere
    (research-1's review, item 16; rgzm). The measurements stay either way."""
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
    """The runtime's sound search, as the model may see it: sound ids and what the map says of each, never a file's
    path (CC-4: a path is the media route's alone)."""
    result = host.live().call("sounds", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    items = result.get("items")
    if isinstance(items, list):
        result["items"] = [{key: value for key, value in item.items() if key != "path"} if isinstance(item, dict)
                           else item for item in items]
    return result


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


# name, schema, handler, emoji: every one registered every time (catalog rule 6).
TOOLS: tuple[tuple[str, dict, Callable[..., dict], str], ...] = (
    ("kumi_status", schemas.KUMI_STATUS, kumi_status, "🎛️"),
    ("kumi_setup", schemas.KUMI_SETUP, kumi_setup, "🧰"),
    ("kumi_connect", schemas.KUMI_CONNECT, kumi_connect, "🔌"),
    ("kumi_observe", schemas.KUMI_OBSERVE, _forward("observe", READ_TIMEOUT), "👀"),
    ("kumi_read", schemas.KUMI_READ, _forward("read", READ_TIMEOUT), "📖"),
    ("kumi_change", schemas.KUMI_CHANGE, kumi_change, "🎚️"),
    ("kumi_undo", schemas.KUMI_UNDO, kumi_undo, "↩️"),
    ("kumi_history", schemas.KUMI_HISTORY, kumi_history, "🕘"),
    ("kumi_listen", schemas.KUMI_LISTEN, kumi_listen, "📈"),
    ("kumi_sounds", schemas.KUMI_SOUNDS, kumi_sounds, "🔎"),
    ("kumi_library", schemas.KUMI_LIBRARY, kumi_library, "🗂️"),
    ("kumi_job", schemas.KUMI_JOB, kumi_job, "⏳"),
    ("kumi_generate", schemas.KUMI_GENERATE, kumi_generate, "🪄"),
)
# What a tool needs before it's shown to the model: kumi_status, kumi_setup and kumi_connect always are (RESEARCH.md
# §5.4).
ALWAYS_SHOWN = {"kumi_status", "kumi_setup", "kumi_connect"}


def approved_only(name: str, args: dict) -> bool:
    """The calls policy.py puts to the producer: they run only as approved (CC-5)."""
    return (name in ("kumi_change", "kumi_undo")
            or (name == "kumi_library" and args.get("action") in ("add_folder", "scan"))
            or (name == "kumi_listen" and is_track_listen(args))
            or (name == "kumi_setup" and args.get("action") in ("install", "adopt"))
            or (name == "kumi_generate" and (args.get("action") == "create"
                                             or (args.get("action") == "quote" and args.get("kind") == "audio_to_midi"))))


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
