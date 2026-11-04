"""Kumi's runtime for the Hermes plugin: which release it pins, and setup (contract C8). Owned by W5.

Stdlib only and import-safe: `dashboard/plugin_api.py` loads this file by path, so it imports no sibling module and
does no I/O at import. Setup is two approved steps (W5-SETUP-INTERFACES.md):

1. **Runtime:** download the bundle `kumi_release.json` pins for this machine, only from Kumi's GitHub release,
   check its size and SHA-256, unpack it into `<data>/runtime/<version>/`, and write `<data>/runtime/current.json`
   last. Nothing else is fetched, and nothing here updates itself: the pin moves only with a new plugin commit
   (Hermes catalog rules 3 and 17).
2. **DAW component:** Kumi's part inside the DAW (Live's Remote Script, REAPER's extension, Renoise's tool), which
   the runtime's `setup_daw` installs. Its dry run names the exact paths, so this step is planned once the runtime
   is installed.

`plan()` records what a step will do; the plugin's approval hook shows `approval()` to the user; `install()` re-plans,
refuses if anything changed since that approval, then does that one step. `<data>` is the active profile's
`plugin-data/kumi`.
"""
from __future__ import annotations

import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import sys
import tarfile
import time
from typing import Any, Callable, Iterator, Mapping
import urllib.error
import urllib.request

PIN_FILE = "kumi_release.json"
REPOSITORY = "user1303836/kumi"
SCHEMA = 1
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
SHA256 = re.compile(r"[0-9a-f]{64}")
DAWS = {"ableton": "Live", "reaper": "REAPER", "renoise": "Renoise"}
# A plan is approved for this long; after that, setup asks again.
PLAN_TTL_MS = 10 * 60 * 1000
DOWNLOAD_TIMEOUT_S = 60
_ARCH = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}
_OS = {"darwin": "apple-darwin", "windows": "pc-windows-msvc", "linux": "unknown-linux-gnu"}
_PLATFORM_NAMES = {"aarch64-apple-darwin": "macOS (Apple silicon)", "x86_64-apple-darwin": "macOS (Intel)",
                   "aarch64-pc-windows-msvc": "Windows (Arm)", "x86_64-pc-windows-msvc": "Windows (x64)",
                   "aarch64-unknown-linux-gnu": "Linux (Arm)", "x86_64-unknown-linux-gnu": "Linux (x64)"}
_COMPONENT_STATES = {"missing", "same", "older", "newer", "other_owner"}


class SetupError(Exception):
    """A setup step that can't or mustn't go ahead; `code` is stable, `message` is for the user."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code, self.message = code, message


def host_target(system: str | None = None, machine: str | None = None) -> str | None:
    """This machine's Rust target triple, as Kumi's release names its bundles, or None where Kumi has no build."""
    arch = _ARCH.get((machine or platform.machine()).lower())
    family = _OS.get((system or platform.system()).lower())
    return f"{arch}-{family}" if arch and family else None


def pinned_version(plugin_dir: Path | None = None) -> str | None:
    """The Kumi version kumi_release.json pins, or None when there is no valid pin file."""
    try:
        pin = json.loads(((plugin_dir or Path(__file__).resolve().parent) / PIN_FILE).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    version = pin.get("kumi") if isinstance(pin, dict) and pin.get("schema") == 1 else None
    return version if isinstance(version, str) and VERSION.fullmatch(version) else None


def executable(target: str) -> str:
    return "kumi.exe" if "windows" in target else "kumi"


def default_data_dir() -> Path:
    """`<hermes home>/plugin-data/kumi`, without creating it (read-only callers must not write)."""
    from hermes_constants import get_hermes_home
    return get_hermes_home() / "plugin-data" / "kumi"


# ---- the pin -------------------------------------------------------------------------------------------------------

def load_pin(plugin_dir: Path | None = None) -> dict:
    """kumi_release.json, refused unless it is schema 1 for one exact version."""
    path = (plugin_dir or Path(__file__).resolve().parent) / PIN_FILE
    try:
        pin = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise SetupError("no_pin", "This copy of the plugin pins no Kumi release yet.") from None
    except (OSError, ValueError) as error:
        raise SetupError("bad_pin", f"The plugin's {PIN_FILE} can't be read: {error}") from None
    if not isinstance(pin, dict) or pin.get("schema") != SCHEMA or not VERSION.fullmatch(str(pin.get("kumi", ""))):
        raise SetupError("bad_pin", f"The plugin's {PIN_FILE} isn't a schema-1 pin of one Kumi version.")
    if not isinstance(pin.get("targets"), dict):
        raise SetupError("bad_pin", f"The plugin's {PIN_FILE} lists no targets.")
    return pin


def pin_for(pin: dict, target: str) -> dict:
    """The pinned download for `target`, checked before anything is fetched: the exact URL on Kumi's release, a
    SHA-256, and sizes."""
    record = pin["targets"].get(target)
    if not isinstance(record, dict):
        raise SetupError("unsupported_platform", f"Kumi {pin['kumi']} has no build for {_PLATFORM_NAMES.get(target, target)}.")
    url = f"https://github.com/{REPOSITORY}/releases/download/v{pin['kumi']}/kumi-{target}.tar.xz"
    if record.get("url") != url:
        raise SetupError("bad_pin", f"The pin's download for {target} isn't {url}.")
    if not isinstance(record.get("sha256"), str) or not SHA256.fullmatch(record["sha256"]):
        raise SetupError("bad_pin", f"The pin has no SHA-256 for {target}.")
    for key in ("size", "unpacked_bytes"):
        if not isinstance(record.get(key), int) or isinstance(record.get(key), bool) or record[key] <= 0:
            raise SetupError("bad_pin", f"The pin has no {key} for {target}.")
    return record


# ---- files setup keeps -----------------------------------------------------------------------------------------------

def _read_json(path: Path) -> dict | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return value if isinstance(value, dict) else None


def _write_json(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    scratch = path.with_name(f".{path.name}.{secrets.token_hex(4)}.tmp")
    scratch.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8", newline="\n")
    os.replace(scratch, path)


def _io_failed(data_dir: Path, error: OSError) -> SetupError:
    return SetupError("io_failed", f"Setup couldn't write its files in {data_dir}: {error.strerror or error}.")


def _now_ms() -> int:
    return int(time.time() * 1000)


def runtime_state(data_dir: Path, version: str | None, target: str | None) -> tuple[str, dict | None]:
    """What `<data>/runtime/current.json` says, checked as runtime.py checks it: `ready`, `missing`, `outdated`
    (another version or target) or `damaged`, with the record."""
    record = _read_json(data_dir / "runtime" / "current.json")
    if record is None:
        return "missing", None
    if record.get("schema") != SCHEMA or not isinstance(record.get("kumi"), str):
        return "damaged", record
    if record.get("version") != version or record.get("target") != target:
        return "outdated", record
    relative = Path(record["kumi"])
    if relative.is_absolute() or ".." in relative.parts:
        return "damaged", record
    binary = data_dir / relative
    try:
        inside = binary.resolve().is_relative_to(data_dir.resolve())
    except OSError:
        inside = False
    if not inside or not binary.is_file() or not os.access(binary, os.X_OK):
        return "damaged", record
    return "ready", record


def existing_kumi(env: Mapping[str, str] | None = None) -> Path | None:
    """A standalone Kumi's home (`KUMI_HOME`, else `~/.kumi`), only by whether the folder exists: setup never reads
    what's inside it."""
    env = os.environ if env is None else env
    home = Path(env["KUMI_HOME"]).expanduser() if env.get("KUMI_HOME") else Path.home() / ".kumi"
    return home if home.is_dir() else None


def _configured_daw() -> str:
    try:
        from hermes_cli.config import load_config_readonly
        entry = ((load_config_readonly() or {}).get("plugins") or {}).get("entries", {}).get("kumi") or {}
        daw = (entry.get("settings") or {}).get("daw")
    except Exception:
        daw = None
    return daw if daw in DAWS else "ableton"


# ---- status: read-only ---------------------------------------------------------------------------------------------

def status(*, daw: str | None = None, data_dir: Path | None = None, plugin_dir: Path | None = None,
           env: Mapping[str, str] | None = None, target: str | None = None) -> dict:
    """Where setup stands, for `/setup`, `kumi_status` and `/kumi`. No network, no writes, no subprocess."""
    daw = daw if daw in DAWS else _configured_daw()
    data_dir = data_dir or default_data_dir()
    target = target or host_target()
    existing = existing_kumi(env)
    receipt = _read_json(data_dir / "setup" / "receipt.json") or {}
    adopted = receipt.get("adopted") if isinstance(receipt.get("adopted"), dict) else None
    result: dict[str, Any] = {
        "schema": SCHEMA, "state": "", "target": target, "pinned_version": None, "download_bytes": None,
        "unpacked_bytes": None, "runtime": {"installed": False}, "daw": daw,
        "daw_component": {"state": "not_installed", "name": None, "path": None, "version": None, "installed_at": None},
        "existing_kumi": {"path": str(existing), "adopted": bool(adopted and Path(adopted.get("kumi_home", "")) == existing)}
        if existing else None,
        "next_step": None, "message": ""}
    try:
        pin = load_pin(plugin_dir)
    except SetupError as error:
        result.update(state="no_pin", message=error.message)
        return result
    result["pinned_version"] = pin["kumi"]
    try:
        record = pin_for(pin, target) if target else None
    except SetupError as error:
        record = None
        if error.code != "unsupported_platform":
            result.update(state="no_pin", message=error.message)
            return result
    if record is None:
        result.update(state="unsupported_platform",
                      message=f"Kumi {pin['kumi']} has no build for this machine ({platform.system()} {platform.machine()}).")
        return result
    result.update(download_bytes=record["size"], unpacked_bytes=record["unpacked_bytes"])
    state, current = runtime_state(data_dir, pin["kumi"], target)
    if current and state != "damaged":
        result["runtime"] = {"installed": True, "version": current.get("version"),
                             "path": str(data_dir / current["kumi"]), "installed_at": current.get("installed_at")}
    if state != "ready":
        label = {"missing": "needs_runtime", "outdated": "outdated_runtime", "damaged": "damaged_runtime"}[state]
        sentence = {"missing": f"Kumi needs its runtime: {_mb(record['size'])} from github.com/{REPOSITORY}.",
                    "outdated": f"This plugin pins Kumi {pin['kumi']}; set up its runtime ({_mb(record['size'])}).",
                    "damaged": "Kumi's runtime is incomplete or was changed; set it up again."}[state]
        result.update(state=label, next_step="install_runtime", message=sentence)
        return result
    component = (receipt.get("daw_components") or {}).get(daw) if isinstance(receipt.get("daw_components"), dict) else None
    if isinstance(component, dict):
        paths = component.get("paths") or []
        result["daw_component"] = {"state": "installed", "name": component.get("component"),
                                   "path": paths[0] if paths else None, "version": component.get("version"),
                                   "installed_at": component.get("installed_at"), "parts": component.get("parts") or []}
    elif adopted:
        result["daw_component"]["state"] = "unknown"
    if result["daw_component"]["state"] == "not_installed":
        result.update(state="needs_daw_component", next_step="install_daw_component",
                      message=f"Kumi's runtime is ready; next, install Kumi's part in {DAWS[daw]}.")
        return result
    connected = (_read_json(data_dir / "state" / "status.json") or {}).get("connected") is True
    after = component.get("after") if isinstance(component, dict) else None
    if connected:
        result.update(state="ready", message=f"Kumi is set up and connected to {DAWS[daw]}.")
    elif after == "choose_control_surface":
        result.update(state="ready", next_step="choose_control_surface",
                      message="In Live, choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI), then connect.")
    else:
        result.update(state="ready", next_step="connect", message=f"Kumi is set up; open {DAWS[daw]} and connect.")
    return result


def _mb(size: int) -> str:
    return f"{size / 1_000_000:.1f} MB"


# ---- plan and approval -----------------------------------------------------------------------------------------------

def _digest(plan: Mapping[str, Any]) -> str:
    canonical = {key: value for key, value in plan.items() if key not in ("digest", "made_at")}
    return hashlib.sha256(json.dumps(canonical, sort_keys=True).encode()).hexdigest()


def _component(result: Any) -> dict:
    """A `setup_daw` result, checked before anything acts on it."""
    if not isinstance(result, dict) or result.get("state") not in _COMPONENT_STATES:
        raise SetupError("bad_runtime_reply", "Kumi's runtime gave setup an answer it doesn't understand.")
    paths, effects = result.get("paths"), result.get("effects")
    if not isinstance(paths, list) or not all(isinstance(p, str) and Path(p).is_absolute() for p in paths):
        raise SetupError("bad_runtime_reply", "Kumi's runtime didn't name the exact paths its DAW step writes.")
    if not isinstance(effects, list) or not all(isinstance(e, str) for e in effects):
        raise SetupError("bad_runtime_reply", "Kumi's runtime didn't describe its DAW step.")
    return result


def _compute_plan(daw: str, setup_daw: Callable[[dict], Any] | None, data_dir: Path, plugin_dir: Path | None,
                  target: str | None) -> dict:
    if daw not in DAWS:
        raise SetupError("bad_daw", f"Kumi supports {', '.join(DAWS)}, not {daw!r}.")
    pin = load_pin(plugin_dir)
    if not target:
        raise SetupError("unsupported_platform", f"Kumi has no build for {platform.system()} {platform.machine()}.")
    record = pin_for(pin, target)
    state, _ = runtime_state(data_dir, pin["kumi"], target)
    existing = existing_kumi()
    plan: dict[str, Any] = {"schema": SCHEMA, "daw": daw, "target": target, "version": pin["kumi"],
                            "existing_kumi": str(existing) if existing else None}
    if state != "ready":
        destination = data_dir / "runtime" / pin["kumi"]
        plan.update(step="runtime", paths=[str(destination)],
                    download={"url": record["url"], "sha256": record["sha256"], "bytes": record["size"],
                              "unpacked_bytes": record["unpacked_bytes"]},
                    effects=[f"Download Kumi {pin['kumi']} for {_PLATFORM_NAMES.get(target, target)} "
                             f"({_mb(record['size'])}) from {record['url']}",
                             "Check it against the SHA-256 this plugin pins, and stop if it differs",
                             f"Unpack it ({_mb(record['unpacked_bytes'])}) into {destination}"])
        return plan
    if setup_daw is None:
        raise SetupError("needs_runtime_client", "Setup needs Kumi's runtime to plan the DAW step.")
    dry = _ask_runtime(setup_daw, {"daw": daw, "dry_run": True}, f"Kumi's runtime couldn't plan the step for {DAWS[daw]}")
    if dry["state"] in ("same", "newer"):
        plan.update(step="none", paths=[], effects=[], component=dry)
    elif dry["state"] == "other_owner":
        plan.update(step="owned_elsewhere", paths=dry["paths"], effects=dry["effects"], component=dry)
    else:
        plan.update(step="daw_component", paths=dry["paths"], effects=dry["effects"], component=dry)
    return plan


def plan(daw: str, *, setup_daw: Callable[[dict], Any] | None = None, data_dir: Path | None = None,
         plugin_dir: Path | None = None, target: str | None = None) -> dict:
    """The next setup step and its exact effects, recorded in `<data>/setup/plan.json` for approval. No network.
    `setup_daw` calls the runtime's `setup_daw` tool; it is needed only once the runtime is installed. Raises only
    SetupError, including when the plan can't be written (`io_failed`)."""
    data_dir = data_dir or default_data_dir()
    result = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target or host_target())
    result["made_at"] = _now_ms()
    result["digest"] = _digest(result)
    try:
        _write_json(data_dir / "setup" / "plan.json", result)
        component = result.get("component") or {}
        if result["step"] == "none" and component.get("state") == "same":  # installed by this Kumi home already
            receipt = _read_json(data_dir / "setup" / "receipt.json") or {}
            if daw not in (receipt.get("daw_components") or {}):
                _record_component(data_dir, daw, component, installed_at=None)
    except OSError as error:
        raise _io_failed(data_dir, error) from None
    return result


def _ask_runtime(setup_daw: Callable[[dict], Any], args: dict, doing: str) -> dict:
    """The runtime's `setup_daw`, its answer checked. The client's own failures (a refusal, a runtime that won't
    start) become SetupError with the runtime's code, so setup's callers handle one kind of error."""
    try:
        return _component(setup_daw(args))
    except SetupError:
        raise
    except Exception as error:
        code = getattr(error, "code", None)
        reason = getattr(error, "reason", None) or str(error) or type(error).__name__
        raise SetupError(code if isinstance(code, str) and code else "daw_step_failed", f"{doing}: {reason}") from None


def _cached_plan(daw: str, data_dir: Path, now: int | None = None, path: Path | None = None) -> dict | None:
    cached = _read_json(path or data_dir / "setup" / "plan.json")
    if not cached or cached.get("daw") != daw or not isinstance(cached.get("made_at"), int):
        return None
    if (now or _now_ms()) - cached["made_at"] > PLAN_TTL_MS or cached.get("digest") != _digest(cached):
        return None
    return cached


def approval(daw: str, *, action: str = "install", data_dir: Path | None = None, now: int | None = None) -> dict | None:
    """What the producer approves for `kumi_setup` `install` or `adopt`: `{"message", "digest"}`, from the recorded
    plan only (no network, no subprocess), or None when there is no fresh plan for `daw` that this action carries out
    (the hook then blocks: "run kumi_setup plan first"). `install()` and `adopt()` carry out only the plan with that
    digest (their `expected_digest`), so what runs is what was approved, even if another chat plans meanwhile."""
    cached = _cached_plan(daw, data_dir or default_data_dir(), now)
    message = _approval_text(cached, daw, action) if cached else None
    return {"message": message, "digest": cached["digest"]} if message else None


def _approval_text(plan: Mapping[str, Any], daw: str, action: str) -> str | None:
    if action == "adopt":
        home = _owner_home(plan)
        if plan.get("step") != "owned_elsewhere" or home is None:
            return None
        return (f"Kumi will use the existing Kumi installation at {home} for its connection to {DAWS[daw]}. Kumi's "
                "runtime will read that installation's DAW connection files and write its own connection state "
                "there (never its sign-in, settings or account), and the "
                f"{plan['component'].get('component') or 'DAW component'} it installed stays as it is.")
    if action != "install" or plan.get("step") not in ("runtime", "daw_component"):
        return None
    return "Kumi setup will: " + "; ".join(plan["effects"]) + "."


def _claim(daw: str, data_dir: Path, expected_digest: str | None, action: str) -> dict:
    """Take the approved plan, so one approval carries out one step: plan.json is renamed away (atomically: a
    second install finds nothing to take), and it must be the fresh plan whose digest the producer approved."""
    if not isinstance(expected_digest, str) or not expected_digest:
        raise SetupError("not_approved", "Setup carries out only a plan the producer approved; plan and approve it.")
    source = data_dir / "setup" / "plan.json"
    taken = source.with_name(f".plan-{secrets.token_hex(4)}.json")
    try:
        os.replace(source, taken)
    except FileNotFoundError:
        raise SetupError("no_plan", "There's no setup plan to carry out (it ran or was replaced); plan again.") from None
    try:
        claimed = _cached_plan(daw, data_dir, path=taken)
    finally:
        taken.unlink(missing_ok=True)
    if claimed is None or claimed.get("digest") != expected_digest:
        raise SetupError("plan_changed", "The setup plan changed after it was approved; plan again and approve that.")
    if _approval_text(claimed, daw, action) is None:
        if action == "install" and claimed.get("step") == "owned_elsewhere":
            raise SetupError("owned_elsewhere", "Another Kumi installation owns this DAW's component; adopt it "
                                                "explicitly (kumi_setup action adopt) or keep them separate.")
        raise SetupError("no_plan", f"The approved plan isn't one {action} carries out; plan again.")
    return claimed


@contextlib.contextmanager
def _setup_lock(data_dir: Path) -> Iterator[None]:
    """One setup step at a time in this profile (two chats, or a chat and `hermes kumi setup`): an exclusive OS lock
    on `<data>/setup/lock` (flock, or msvcrt on Windows), which the OS releases if the process dies. Never waits."""
    path = data_dir / "setup" / "lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a+b") as handle:  # the file stays; only the lock on it comes and goes
        try:
            if os.name == "nt":
                import msvcrt
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise SetupError("busy", "Another Kumi setup step is running in this profile; try again when it ends.") from None
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _unstamped(plan: Mapping[str, Any]) -> dict:
    return {key: value for key, value in plan.items() if key not in ("digest", "made_at")}


def _owner_home(plan: Mapping[str, Any]) -> str | None:
    owner = (plan.get("component") or {}).get("owner")
    home = owner.get("kumi_home") if isinstance(owner, dict) else None
    return home if isinstance(home, str) and Path(home).is_absolute() else None


def adopt(daw: str, *, expected_digest: str | None, setup_daw: Callable[[dict], Any] | None = None,
          data_dir: Path | None = None, plugin_dir: Path | None = None, target: str | None = None) -> dict:
    """Adopt the Kumi installation that owns this DAW's component, after the user approved `approval(daw,
    action="adopt")` (`expected_digest` is that approval's digest): from now on the runtime runs with `--kumi-home
    <that folder>`, so it uses the bridge that is already installed instead of replacing it. Re-plans first and
    refuses if anything changed. Setup itself never reads that installation's files, account or settings: it records
    the folder in `current.json` and the receipt. A runtime already running keeps its old home until W2 restarts it.
    Returns `{"ok": ...}` like `install()`; never raises."""
    data_dir = data_dir or default_data_dir()
    target = target or host_target()
    try:
        with _setup_lock(data_dir):
            approved = _claim(daw, data_dir, expected_digest, "adopt")
            current = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target)
            if _digest(current) != _digest(_unstamped(approved)):
                raise SetupError("plan_changed", "What setup found changed since it was approved; plan again.")
            home = _owner_home(approved)
            if not Path(home).is_dir():
                raise SetupError("not_found", f"{home} isn't a folder any more; plan again.")
            _adopt(approved, daw, home, data_dir)
        return {"ok": True, "step": "adopt",
                "message": f"Kumi now uses the Kumi installation at {home} for {DAWS[daw]}.",
                "status": status(daw=daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)}
    except SetupError as error:
        return {"ok": False, "error": {"code": error.code, "message": error.message}}
    except OSError as error:
        failure = _io_failed(data_dir, error)
        return {"ok": False, "error": {"code": failure.code, "message": failure.message}}


def _adopt(approved: Mapping[str, Any], daw: str, home: str, data_dir: Path) -> None:
    """Record the adopted Kumi home: in `current.json`, which runtime.py starts the runtime with, and the receipt."""
    record = _read_json(data_dir / "runtime" / "current.json")
    if not record:
        raise SetupError("needs_runtime", "Install Kumi's runtime before adopting an installation.")
    _write_json(data_dir / "runtime" / "current.json", {**record, "kumi_home": home})
    receipt = _read_json(data_dir / "setup" / "receipt.json") or {"schema": SCHEMA}
    component = approved["component"]
    receipt["adopted"] = {"kumi_home": home, "daw": daw, "at": _now_ms()}
    components = receipt.get("daw_components") if isinstance(receipt.get("daw_components"), dict) else {}
    components[daw] = {"component": component.get("component"), "version": component.get("version"),
                       "paths": component.get("paths") or [], "after": None, "adopted": True, "installed_at": None}
    receipt["daw_components"] = components
    _write_json(data_dir / "setup" / "receipt.json", receipt)


# ---- install ---------------------------------------------------------------------------------------------------------

def install(daw: str, *, expected_digest: str | None, setup_daw: Callable[[dict], Any] | None = None,
            progress: Callable[[int, int], None] | None = None, data_dir: Path | None = None,
            plugin_dir: Path | None = None, target: str | None = None,
            opener: Callable[..., Any] | None = None) -> dict:
    """Do the step the user approved, and only that: the plan whose digest `approval()` gave (`expected_digest`),
    taken so it runs once, re-planned and refused if anything changed since, one setup step at a time. Returns
    `{"ok": True, "step", "status", "message"}` or `{"ok": False, "error": {"code", "message"}}`; never raises."""
    data_dir = data_dir or default_data_dir()
    target = target or host_target()
    try:
        with _setup_lock(data_dir):
            approved = _claim(daw, data_dir, expected_digest, "install")
            current = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target)
            if _digest(current) != _digest(_unstamped(approved)):
                raise SetupError("plan_changed", "What setup would do changed since it was approved; plan again.")
            if approved["step"] == "runtime":
                message = _install_runtime(approved, data_dir, target, progress, opener or _open_url)
            else:
                message = _install_component(approved, setup_daw, data_dir)
        return {"ok": True, "step": approved["step"], "message": message,
                "status": status(daw=daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)}
    except SetupError as error:
        return {"ok": False, "error": {"code": error.code, "message": error.message}}
    except OSError as error:
        failure = _io_failed(data_dir, error)
        return {"ok": False, "error": {"code": failure.code, "message": failure.message}}


def _open_url(url: str) -> Any:
    request = urllib.request.Request(url, headers={"User-Agent": "kumi-hermes-setup"})
    return urllib.request.urlopen(request, timeout=DOWNLOAD_TIMEOUT_S)


def _download(url: str, expect_size: int, expect_sha256: str, into: Path,
              progress: Callable[[int, int], None] | None, opener: Callable[..., Any]) -> None:
    digest, done = hashlib.sha256(), 0
    try:
        with opener(url) as response, into.open("wb") as out:
            while chunk := response.read(1 << 20):
                done += len(chunk)
                if done > expect_size:
                    raise SetupError("checksum_mismatch", "The download is larger than the pinned bundle; stopped.")
                digest.update(chunk)
                out.write(chunk)
                if progress:
                    progress(done, expect_size)
    except (urllib.error.URLError, http.client.HTTPException, OSError, TimeoutError) as error:
        raise SetupError("download_failed", f"Downloading Kumi failed: {error}") from None
    if done != expect_size or digest.hexdigest() != expect_sha256:
        raise SetupError("checksum_mismatch", "The downloaded bundle doesn't match the SHA-256 this plugin pins; "
                                              "nothing was installed.")


def _check_members(archive: tarfile.TarFile, target: str) -> None:
    members = archive.getmembers()
    for member in members:
        parts = Path(member.name).parts
        if member.name.startswith(("/", "\\")) or ".." in parts or not (member.isfile() or member.isdir()):
            raise SetupError("bad_bundle", f"The bundle holds {member.name!r}, which setup won't unpack.")
    if not any(member.isfile() and member.name.removeprefix("./") == executable(target) for member in members):
        raise SetupError("bad_bundle", f"The bundle has no {executable(target)} at its root.")


def _install_runtime(approved: dict, data_dir: Path, target: str, progress: Callable[[int, int], None] | None,
                     opener: Callable[..., Any]) -> str:
    version, download = approved["version"], approved["download"]
    runtime = data_dir / "runtime"
    runtime.mkdir(parents=True, exist_ok=True)
    token = secrets.token_hex(4)
    archive_path, staging = runtime / f".download-{token}.tar.xz", runtime / f".staging-{version}-{token}"
    final, old = runtime / version, runtime / f".old-{version}-{token}"
    try:
        _download(download["url"], download["bytes"], download["sha256"], archive_path, progress, opener)
        if not hasattr(tarfile, "data_filter"):
            raise SetupError("python_too_old", "Setup needs Python 3.11.4 or later to unpack the bundle safely.")
        try:
            with tarfile.open(archive_path, "r:xz") as archive:
                _check_members(archive, target)
                staging.mkdir()
                archive.extractall(staging, filter="data")
        except (tarfile.TarError, EOFError) as error:
            raise SetupError("bad_bundle", f"The bundle can't be unpacked: {error}") from None
        binary = staging / executable(target)
        if not binary.is_file():
            raise SetupError("bad_bundle", f"The bundle has no {executable(target)}.")
        if os.name != "nt":
            binary.chmod(binary.stat().st_mode | 0o700)
        if final.exists():
            final.rename(old)
        staging.rename(final)
        previous = _read_json(runtime / "current.json") or {}
        current = {"schema": SCHEMA, "version": version, "target": target,
                   "kumi": f"runtime/{version}/{executable(target)}", "sha256": download["sha256"],
                   "installed_at": _now_ms()}
        if isinstance(previous.get("kumi_home"), str):
            current["kumi_home"] = previous["kumi_home"]
        _write_json(runtime / "current.json", current)
        receipt = _read_json(data_dir / "setup" / "receipt.json") or {"schema": SCHEMA}
        receipt["runtime"] = {"version": version, "target": target, "sha256": download["sha256"],
                              "url": download["url"], "installed_at": current["installed_at"]}
        _write_json(data_dir / "setup" / "receipt.json", receipt)
    finally:
        archive_path.unlink(missing_ok=True)
        shutil.rmtree(staging, ignore_errors=True)
        shutil.rmtree(old, ignore_errors=True)
    for stale in runtime.iterdir():  # earlier versions this plugin installed; a running one keeps its open files
        if stale.is_dir() and stale.name != version and VERSION.fullmatch(stale.name):
            shutil.rmtree(stale, ignore_errors=True)
    return f"Kumi {version}'s runtime is installed in {final}."


def _record_component(data_dir: Path, daw: str, component: Mapping[str, Any], *, installed_at: int | None) -> None:
    """What `setup_daw` reported for `daw`, in the receipt that status() reads."""
    receipt = _read_json(data_dir / "setup" / "receipt.json") or {"schema": SCHEMA}
    components = receipt.get("daw_components") if isinstance(receipt.get("daw_components"), dict) else {}
    parts = [part for part in component.get("parts") or [] if isinstance(part, dict)]
    components[daw] = {"component": component.get("component"), "version": component.get("version"),
                       "paths": component.get("paths") or [], "after": component.get("after"), "parts": parts,
                       "installed_at": installed_at}
    receipt["daw_components"] = components
    _write_json(data_dir / "setup" / "receipt.json", receipt)


def _install_component(approved: dict, setup_daw: Callable[[dict], Any] | None, data_dir: Path) -> str:
    if setup_daw is None:
        raise SetupError("needs_runtime_client", "Setup needs Kumi's runtime for the DAW step.")
    done = _ask_runtime(setup_daw, {"daw": approved["daw"], "dry_run": False},
                        f"Installing Kumi's part in {DAWS[approved['daw']]} failed")
    _record_component(data_dir, approved["daw"], done, installed_at=done.get("installed_at") or _now_ms())
    after = {"choose_control_surface": " In Live, choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI).",
             "restart_daw": f" Restart {DAWS[approved['daw']]} to load it.",
             "enable_tool": " Enable the Kumi tool in Renoise's Tool Browser."}.get(done.get("after") or "", "")
    return f"Kumi's part in {DAWS[approved['daw']]} is installed.{after}"


# ---- `hermes kumi setup` ---------------------------------------------------------------------------------------------

def cli_main(daw: str | None = None, *, setup_daw: Callable[[dict], Any] | None = None,
             data_dir: Path | None = None, plugin_dir: Path | None = None, target: str | None = None,
             opener: Callable[..., Any] | None = None, stdin: Any = None, stdout: Any = None) -> int:
    """`hermes kumi setup`: show each step, ask before it, refuse without a terminal. There is no --yes."""
    stdin, stdout = stdin or sys.stdin, stdout or sys.stdout
    if not (stdin.isatty() and stdout.isatty()):
        print("hermes kumi setup asks before each step, so it needs a terminal; in a chat, ask Hermes to set up Kumi.",
              file=stdout)
        return 1
    daw = daw if daw in DAWS else _configured_daw()
    for _ in range(3):  # runtime, then the DAW component, then a final check
        try:
            step = plan(daw, setup_daw=setup_daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)
        except SetupError as error:
            print(error.message, file=stdout)
            return 1
        if step["step"] == "none":
            print(status(daw=daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)["message"], file=stdout)
            return 0
        adopting = step["step"] == "owned_elsewhere"
        if adopting:
            text = _approval_text(step, daw, "adopt")
            if text is None:
                print("Another Kumi installation owns this DAW's component:", *step["effects"], sep="\n  ", file=stdout)
                return 1
            print(text, file=stdout)
        else:
            print("Kumi setup will:", *step["effects"], sep="\n  ", file=stdout)
        print("Go ahead? [y/N] ", end="", file=stdout, flush=True)
        if stdin.readline().strip().lower() not in ("y", "yes"):
            print("Nothing was changed.", file=stdout)
            return 1
        if adopting:
            result = adopt(daw, expected_digest=step["digest"], setup_daw=setup_daw, data_dir=data_dir,
                           plugin_dir=plugin_dir, target=target)
        else:
            result = install(daw, expected_digest=step["digest"], setup_daw=setup_daw, data_dir=data_dir,
                             plugin_dir=plugin_dir, target=target, opener=opener)
        if not result["ok"]:
            print(result["error"]["message"], file=stdout)
            return 1
        print(result["message"], file=stdout)
        if adopting:
            return 0
        if step["step"] == "runtime" and setup_daw is None:
            return 0
    return 0
