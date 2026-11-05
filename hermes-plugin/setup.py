"""Kumi's runtime for the Hermes plugin: which release it pins, and setup (contract C8). Owned by W5.

Stdlib only and import-safe: `dashboard/plugin_api.py` loads this file by path, so it imports no sibling module and
does no I/O at import. Setup is two approved steps and an optional third (W5-SETUP-INTERFACES.md, CC-10):

1. **Runtime:** download the bundle `kumi_release.json` pins for this machine, only from Kumi's GitHub release,
   check its size and SHA-256, unpack it into `<data>/runtime/<version>/`, and write `<data>/runtime/current.json`
   last. Nothing else is fetched, and nothing here updates itself: the pin moves only with a new plugin commit
   (Hermes catalog rules 3 and 17).
2. **DAW component:** Kumi's part inside the DAW (Live's Remote Script, REAPER's extension, Renoise's tool), which
   the runtime's `setup_daw` installs. Its dry run names the exact paths, so this step is planned once the runtime
   is installed.
3. **ffmpeg (optional):** an ffmpeg already on this computer, which setup only looks for, recorded with its identity
   in `current.json` so the runtime can decode MP3, FLAC and other compressed audio. Without it, WAV and AIFF only.

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
import posixpath
import re
import secrets
import shutil
import stat
import sys
import tarfile
import time
from typing import Any, Callable, Iterator, Mapping
import unicodedata
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


def platform_name(target: Any) -> str:
    """A build target as a producer knows it: "macOS (Apple silicon)" for aarch64-apple-darwin."""
    return _PLATFORM_NAMES.get(target, str(target))


_COMPONENT_STATES = {"missing", "same", "older", "newer", "other_owner"}
# Setup shows the producer its lines verbatim, in the chat's approval and in the terminal, so it refuses text it
# couldn't show as it is: longer than a path can be, more lines than an approval should hold, or characters that hide,
# reorder or redraw text (controls; bidi, zero-width, BOM and tag characters; lone surrogates; line separators).
_MAX_PATH, _MAX_LINE, _MAX_LINES = 1024, 1280, 24
_HIDDEN = {"Cc", "Cf", "Cs", "Zl", "Zp"}


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
        raise SetupError("unsupported_platform", f"Kumi {pin['kumi']} has no build for {platform_name(target)}.")
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


# ---- ffmpeg (CC-10): an optional program the producer allows; setup only looks for it ------------------------------

_FFMPEG_FOLDERS = {"darwin": ("/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"),
                   "linux": ("/usr/bin", "/usr/local/bin", "/snap/bin")}
_IDENTITY = ("path", "size", "mtime_ns", "ino")
_WINDOWS_LOCAL = re.compile(r"[A-Za-z]:[\\/]")
_DRIVE_FIXED = 3  # GetDriveTypeW's local disk: not removable, a network drive, a CD or a RAM disk
# Where the automounter mounts a server's share on first touch: `/net`, macOS's `/Network`, both again under macOS's
# firmlinked `/System/Volumes/Data`, and on macOS `/home` (auto_home; Linux's is a plain folder). snapshots.local keeps
# the same list.
_AUTOMOUNTS = ("/net/", "/network/", "/system/volumes/data/net/", "/system/volumes/data/network/")
_AUTO_HOME = ("/home/", "/system/volumes/data/home/")


def _local(path: str, windows: bool | None = None) -> bool:
    """Whether `path` is a local absolute path, judged from its text before anything touches it: on Windows a drive
    letter and a root, never UNC (`\\\\server\\share`) or a device namespace (`\\\\?\\`, `\\\\.\\`), which could make
    Windows reach a network share before anything is approved; elsewhere `/…`, never `//…`."""
    if os.name == "nt" if windows is None else windows:
        return _WINDOWS_LOCAL.match(path) is not None
    return path.startswith("/") and not path.startswith("//")


def _drive_type(root: str) -> int:
    """Windows's kind of drive for `root` (`C:\\`), from its drive table: it needn't reach the drive to tell."""
    import ctypes
    return ctypes.windll.kernel32.GetDriveTypeW(ctypes.c_wchar_p(root))


def _on_this_computer(path: str, windows: bool | None = None, darwin: bool | None = None) -> bool:
    """Whether `path` can be touched without reaching another computer: `_local()`, then on Windows a fixed drive
    (never a mapped network drive such as `Z:\\`), elsewhere nothing under an automount folder (`_AUTOMOUNTS`)."""
    windows = os.name == "nt" if windows is None else windows
    if not _local(path, windows):
        return False
    if windows:
        return _drive_type(path[0].upper() + ":\\") == _DRIVE_FIXED
    folded = (posixpath.normpath(path) + "/").casefold()
    darwin = sys.platform == "darwin" if darwin is None else darwin
    return not folded.startswith(_AUTOMOUNTS) and not (darwin and folded.startswith(_AUTO_HOME))


def _ffmpeg_search(env: Mapping[str, str] | None = None, platform_name: str | None = None) -> list[str]:
    """The folders setup looks in for ffmpeg, in order: each `PATH` entry that is a local absolute path (never an
    empty or relative entry, `.`, or the working folder, which `shutil.which` may search on Windows), then the usual
    install folders."""
    env = os.environ if env is None else env
    platform_name = platform_name or sys.platform
    windows = platform_name == "win32"
    entries = [entry.strip().strip('"') for entry in (env.get("PATH") or "").split(";" if windows else ":")]
    folders: list[str] = []
    for folder in [*entries, *_FFMPEG_FOLDERS.get(platform_name, ())]:
        if folder and _local(folder, windows) and folder not in folders:
            folders.append(folder)
    return folders


def _resolved(path: str) -> str | None:
    """`path` with every link in it followed from the root down, as `realpath` would (a Homebrew link becomes its
    Cellar file), except that nothing is looked up before it's known to be on this computer: each step on the way (a
    relative link can climb into `/net`), and each link's target, read without following it (`readlink`). A folder
    linked to a network share is never reached. None if a step or target isn't, or the links loop. snapshots.follow
    keeps the same rules (the dashboard loads each file on its own, so neither can import the other)."""
    if not _on_this_computer(path):
        return None
    anchor, *names = Path(path).parts
    resolved, hops = anchor, 0
    while names:
        name = names.pop(0)
        if name in ("", "."):
            continue
        if name == "..":
            resolved = os.path.dirname(resolved)
            continue
        step = os.path.join(resolved, name)
        if not _on_this_computer(step):  # before its lstat: looking up a network root mounts it
            return None
        try:  # lstat and readlink: neither follows `step`, and everything above it is already resolved
            target = os.readlink(step) if _linked(step) else None
        except (OSError, ValueError):
            return None
        if target is None:
            resolved = step
            continue
        if target.startswith("\\\\?\\") and _local(target[4:], windows=True):  # Windows's long form of a local path
            target = target[4:]
        hops, linked_to = hops + 1, Path(target)
        if hops > 32 or (linked_to.anchor and not linked_to.is_absolute()):  # a loop, or `\x` or `C:x` on Windows
            return None
        if linked_to.is_absolute():
            if not _on_this_computer(target):
                return None
            resolved, names = linked_to.anchor, [*linked_to.parts[1:], *names]
        else:
            names = [*linked_to.parts, *names]
    if not _on_this_computer(resolved):
        return None
    real = os.path.realpath(resolved)  # no links are left: this only spells it canonically (Windows: 8.3 names, case)
    return real if _on_this_computer(real) else None


def _linked(path: str) -> bool:
    """Whether `path` is a link, or on Windows a junction (a mount-point reparse point, which `os.path.isjunction` sees
    only from Python 3.12), from one `lstat` that follows neither. A path that isn't there is no link."""
    try:
        info = os.lstat(path)
    except (OSError, ValueError):
        return False
    return stat.S_ISLNK(info.st_mode) or (getattr(info, "st_reparse_tag", 0) != 0
                                          and info.st_reparse_tag == getattr(stat, "IO_REPARSE_TAG_MOUNT_POINT", -1))


def _identity(path: str) -> dict | None:
    """The program at `path` as an approval binds it: `{path, size, mtime_ns, ino}` from one `stat`, if it's a
    regular, executable file. runtime.py passes it to the runtime only while all four still match."""
    try:
        info = os.stat(path)
    except (OSError, ValueError):
        return None
    if not stat.S_ISREG(info.st_mode) or not os.access(path, os.X_OK):
        return None
    return {"path": path, "size": info.st_size, "mtime_ns": info.st_mtime_ns, "ino": info.st_ino}


def find_ffmpeg(env: Mapping[str, str] | None = None) -> dict | None:
    """An ffmpeg already on this computer, found only by looking: the first in `_ffmpeg_search()` that resolves to a
    local, regular, executable file, as `_identity()` of its canonical path. Setup never runs it and never downloads
    one (catalog rule 17)."""
    name = "ffmpeg.exe" if os.name == "nt" else "ffmpeg"
    for folder in _ffmpeg_search(env):
        path = _resolved(os.path.join(folder, name))
        found = _identity(path) if path and _plain(path, _MAX_PATH) else None
        if found:
            return found
    return None


def _allowed_ffmpeg(data_dir: Path) -> tuple[str, str | None]:
    """The ffmpeg the producer allowed (`tools.ffmpeg` in `current.json`), and whether it's still that program, as
    runtime.py checks at each start: `available`, `changed`, `gone` or `not_allowed`. A path that now leads elsewhere
    through a link is `changed`, and isn't touched."""
    tools = (_read_json(data_dir / "runtime" / "current.json") or {}).get("tools")
    allowed = tools.get("ffmpeg") if isinstance(tools, dict) else None
    path = allowed.get("path") if isinstance(allowed, dict) else None
    if not isinstance(path, str) or not _local(path):
        return "not_allowed", None
    if _resolved(path) != path:
        return "changed", path
    if _identity(path) == {key: allowed.get(key) for key in _IDENTITY}:
        return "available", path
    return ("changed" if os.path.lexists(path) else "gone"), path


def compressed_audio(data_dir: Path, env: Mapping[str, str] | None = None) -> dict:
    """Whether Kumi's runtime can decode MP3, FLAC and other compressed audio: `{"state": "available" | "changed" |
    "gone" | "not_allowed", "ffmpeg": the allowed path, "found": an ffmpeg setup could offer, "message"}`. `message`
    is the line `hermes kumi status` shows (`for_model` says it without the paths). No subprocess."""
    state, path = _allowed_ffmpeg(data_dir)
    found = None if state == "available" else find_ffmpeg(env)
    offer = found["path"] if found else None
    return {"state": state, "ffmpeg": path, "found": offer, "message": _audio_line(state, path, offer)}


def _audio_line(state: str, path: str | None, offer: str | None, *, paths: bool = True) -> str:
    """`compressed_audio`'s line; without `paths`, the same line naming no path on this computer."""
    if state == "available":
        return f"Compressed audio (MP3, FLAC…): yes, with the ffmpeg {f'at {path}' if paths else 'you allowed'}."
    why = {"changed": f"The ffmpeg at {path} changed since you allowed it" if paths else
           "The ffmpeg you allowed has changed since",
           "gone": f"The ffmpeg you allowed ({path}) is gone" if paths else "The ffmpeg you allowed is gone"
           }.get(state, "Kumi reads WAV and AIFF only")
    if offer:
        other = f"let Kumi use the ffmpeg at {offer}" if paths else "let Kumi use the ffmpeg setup found"
        allow = "allow it again" if offer == path else other
        message = f"{why}; to {allow}, run hermes kumi setup (or ask Hermes to set up Kumi)."
    elif state == "not_allowed":
        message = f"{why}: setup found no ffmpeg on this computer."
    else:
        message = f"{why}, and setup found no other ffmpeg on this computer; Kumi reads WAV and AIFF only."
    return f"Compressed audio (MP3, FLAC…): no. {message}"


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
        "compressed_audio": compressed_audio(data_dir, env), "next_step": None, "message": ""}
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
                    "outdated": f"This plugin needs Kumi {pin['kumi']}; setup downloads it ({_mb(record['size'])}).",
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
                      message=f"Kumi's runtime is ready; next, setup installs Kumi's part in {DAWS[daw]}.")
        return result
    connected = (_read_json(data_dir / "state" / "status.json") or {}).get("connected") is True
    after = component.get("after") if isinstance(component, dict) else None
    if connected:
        result.update(state="ready", message=f"Kumi is set up and connected to {DAWS[daw]}.")
    elif after == "choose_control_surface":
        result.update(state="ready", next_step="choose_control_surface",
                      message="In Live, choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI), "
                              "then ask Hermes to connect to it.")
    else:
        result.update(state="ready", next_step="connect",
                      message=f"Kumi is set up: open {DAWS[daw]}, then ask Hermes to connect to it.")
    return result


def for_model(report: Mapping[str, Any]) -> dict:
    """`status()` as `kumi_status` gives it to the model: where setup stands and what's next, with no path on this
    computer in it (the producer's home, the runtime's and the DAW's folders, where ffmpeg is), which the model has no
    use for and a chat's transcript shouldn't carry (adversarial review LOW 11). `hermes kumi status` shows them."""
    shown = {key: report.get(key) for key in ("schema", "state", "target", "pinned_version", "download_bytes",
                                               "unpacked_bytes", "daw", "next_step", "message")}
    if report.get("state") == "no_pin":  # a pin that can't be read says why in words that may name its file
        shown["message"] = "This copy of the plugin has no Kumi release it can set up; hermes kumi status says why."
    runtime = report.get("runtime") or {}
    shown["runtime"] = {key: runtime[key] for key in ("installed", "version", "installed_at") if key in runtime}
    component = report.get("daw_component") or {}
    shown["daw_component"] = {key: component.get(key) for key in ("state", "name", "version", "installed_at")}
    parts = [{"name": part.get("name"), "state": part.get("state")} for part in component.get("parts") or []
             if isinstance(part, dict)]
    if parts:
        shown["daw_component"]["parts"] = parts
    existing = report.get("existing_kumi")
    shown["existing_kumi"] = {"adopted": existing.get("adopted") is True} if isinstance(existing, dict) else None
    audio = report.get("compressed_audio") or {}
    shown["compressed_audio"] = {"state": audio.get("state"), "message": _audio_line(
        str(audio.get("state") or ""), audio.get("ffmpeg"), audio.get("found"), paths=False)}
    return shown


def plan_for_model(plan: Mapping[str, Any]) -> dict:
    """A plan as `kumi_setup plan` gives it to the model: the step, what it does in words and its size, with no path or
    URL (adversarial review LOW 11). The producer's approval shows every one of them, from the plan setup recorded
    (`approval()`); the model has no use for them, and a chat's transcript shouldn't carry them."""
    shown = {key: plan[key] for key in ("schema", "daw", "target", "version", "step", "optional", "digest")
             if key in plan}
    download = plan.get("download") or {}
    if download:
        shown.update(download_bytes=download.get("bytes"), unpacked_bytes=download.get("unpacked_bytes"))
    component = plan.get("component") or {}
    if component:
        shown["component"] = {key: component.get(key) for key in ("component", "version", "state")}
    shown["existing_kumi"] = plan.get("existing_kumi") is not None
    daw = DAWS.get(str(plan.get("daw")), "the DAW")
    shown["summary"] = {
        "runtime": f"Download Kumi {plan.get('version')} ({_mb(int(download.get('bytes') or 0))}), check it against "
                   "the SHA-256 this plugin pins, and unpack it into this profile's plugin data.",
        "daw_component": f"Install Kumi's part in {daw}; the approval lists everything it writes.",
        "owned_elsewhere": f"Another Kumi installation already serves {daw}: adopt it rather than replace it.",
        "ffmpeg": "Optional: let Kumi's runtime use the ffmpeg already on this computer for compressed audio.",
        "use_daw": f"Make {daw} Kumi's DAW in this profile, so Kumi connects to it.",
        "none": f"Kumi is set up for {daw}.",
    }.get(str(plan.get("step")), "")
    shown["use_daw"] = isinstance(plan.get("use_daw"), str)
    if shown["use_daw"] and plan.get("step") != "use_daw":
        shown["summary"] += f" The same approval makes {daw} Kumi's DAW in this profile."
    return shown


# A setup failure's own words can name a path or a URL; the model is told this instead.
_PATHLIKE = re.compile(r"[/\\]")


def reason_for_model(code: str, message: Any) -> str:
    """A setup failure as `kumi_setup` gives it to the model: its own words, unless they could name a path or a URL
    (LOW 11), which `hermes kumi setup` in a terminal shows instead."""
    if isinstance(message, str) and message and not _PATHLIKE.search(message):
        return message
    return f"Setup failed ({code}); hermes kumi setup in a terminal says why."


def _mb(size: int) -> str:
    return f"{size / 1_000_000:.1f} MB"


# ---- plan and approval -----------------------------------------------------------------------------------------------

def _digest(plan: Mapping[str, Any]) -> str:
    canonical = {key: value for key, value in plan.items() if key not in ("digest", "made_at")}
    return hashlib.sha256(json.dumps(canonical, sort_keys=True).encode()).hexdigest()


def _plain(text: Any, limit: int) -> bool:
    """Whether `text` can be shown to the producer exactly as it is (see `_HIDDEN`). Setup refuses anything else rather
    than clean it: a cleaned path isn't the path written."""
    return (isinstance(text, str) and 0 < len(text) <= limit
            and not any(unicodedata.category(char) in _HIDDEN for char in text))


def _component(result: Any) -> dict:
    """A `setup_daw` result, checked before anything acts on it or shows it."""
    if not isinstance(result, dict) or result.get("state") not in _COMPONENT_STATES:
        raise SetupError("bad_runtime_reply", "Kumi's runtime gave setup an answer it doesn't understand.")
    paths, effects = result.get("paths"), result.get("effects")
    if not isinstance(paths, list) or not all(_plain(p, _MAX_PATH) and Path(p).is_absolute() for p in paths):
        raise SetupError("bad_runtime_reply", "Kumi's runtime didn't name the exact paths its DAW step writes, as "
                                              "plain absolute paths setup can show.")
    if not isinstance(effects, list) or not all(_plain(e, _MAX_LINE) for e in effects):
        raise SetupError("bad_runtime_reply", "Kumi's runtime didn't describe its DAW step in plain lines setup can show.")
    if len(paths) + len(effects) > _MAX_LINES:
        raise SetupError("bad_runtime_reply", f"Kumi's runtime described its DAW step in more than {_MAX_LINES} lines.")
    owner = result.get("owner")
    home = owner.get("kumi_home") if isinstance(owner, dict) else None
    named = [result.get(key) for key in ("component", "version") if result.get(key) is not None]
    if (home is not None and not _plain(home, _MAX_PATH)) or not all(_plain(name, 200) for name in named):
        raise SetupError("bad_runtime_reply", "Kumi's runtime named its DAW component or its owner in text setup "
                                              "can't show.")
    digest = result.get("digest")
    if digest is not None and not (isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)):
        raise SetupError("bad_runtime_reply", "Kumi's runtime gave its DAW step a digest setup can't check.")
    return result


# A step that changes what the runtime runs with stops this Hermes's runtime for the profile (the `kumi_setup` handler
# does), which ends a chat's DAW connection until its next Kumi call: the approval says so.
_RESTART = ("If Kumi's runtime is running in this Hermes, it stops: a chat connected to your DAW reconnects at its next "
            "Kumi call")


def _compute_plan(daw: str, setup_daw: Callable[[dict], Any] | None, data_dir: Path, plugin_dir: Path | None,
                  target: str | None, current: str | None = None, use: bool = False) -> dict:
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
                    effects=[f"Download Kumi {pin['kumi']} for {platform_name(target)} "
                             f"({_mb(record['size'])}) from {record['url']}",
                             "Check it against the SHA-256 this plugin pins, and stop if it differs",
                             f"Unpack it ({_mb(record['unpacked_bytes'])}) into {destination}", _RESTART])
        return plan
    if setup_daw is None:
        raise SetupError("needs_runtime_client", "Setup needs Kumi's runtime to plan the DAW step.")
    dry = _ask_runtime(setup_daw, {"daw": daw, "dry_run": True}, f"Kumi's runtime couldn't plan the step for {DAWS[daw]}")
    using = _use_line(daw, current) if _switches(daw, current, use, data_dir) else None
    if dry["state"] in ("same", "newer") and using:  # the DAW is set up; making it Kumi's DAW comes before ffmpeg
        plan.update(step="use_daw", paths=[], effects=[], component=dry, use_daw=using)
    elif dry["state"] in ("same", "newer"):
        found = None if _allowed_ffmpeg(data_dir)[0] == "available" else find_ffmpeg()
        if found is None:
            plan.update(step="none", paths=[], effects=[], component=dry)
        else:  # the DAW is set up; offering ffmpeg is the last, optional step
            plan.update(step="ffmpeg", optional=True, paths=[], ffmpeg=found, component=dry, effects=[
                f"Kumi's runtime will run {found['path']} to decode MP3, FLAC and other compressed audio in the "
                "folders and files you let it read",
                "Kumi doesn't download or install ffmpeg. Without it, Kumi reads WAV and AIFF only.", _RESTART])
    elif dry["state"] == "other_owner":
        plan.update(step="owned_elsewhere", paths=dry["paths"], effects=[*dry["effects"], *_tagging(daw, dry)],
                    component=dry, **({"use_daw": using} if using else {}))
    else:
        plan.update(step="daw_component", paths=dry["paths"], effects=[*dry["effects"], *_tagging(daw, dry)],
                    component=dry, **({"use_daw": using} if using else {}))
    return plan


def _set_up(daw: str, data_dir: Path) -> bool:
    """Whether this profile's setup has `daw`'s part in place (its receipt names it)."""
    components = (_read_json(data_dir / "setup" / "receipt.json") or {}).get("daw_components")
    return isinstance(components, dict) and daw in components


def _switches(daw: str, current: str | None, use: bool, data_dir: Path) -> bool:
    """Whether setting `daw` up also makes it Kumi's DAW in this profile (the plugin's `daw` setting, now `current`):
    when the producer asks (`use`), or when `current`'s part isn't set up here, so `daw` is the first DAW set up. A
    second DAW never takes the place of one that's set up, unasked (kumi-adv)."""
    return current in DAWS and current != daw and (use or not _set_up(current, data_dir))


def _use_line(daw: str, current: str) -> str:
    return f"Make {DAWS[daw]} Kumi's DAW in this profile, instead of {DAWS[current]}: Kumi then connects to {DAWS[daw]}"


def next_daw(daw: str, current: str | None) -> str:
    """The exact next step when `daw` is set up but another DAW stays Kumi's (said after setup, never done unasked)."""
    if current not in DAWS or current == daw:
        return ""
    return (f"Kumi still connects to {DAWS[current]} in this profile; to use {DAWS[daw]}, run hermes kumi setup "
            f"--daw {daw} --use.")


# CC-8: what REAPER's and Renoise's components write into a project by themselves, said in their DAW step.
_TAGGING = {
    "reaper": ("REAPER: Kumi's extension keeps an id for each saved project in the project (ext state), saved with it.",),
    "renoise": ("Renoise: Kumi's tool keeps an id for each song in the song's tool data, saved with it.",),
}
# A component that reports `keep_identity: true` (8c) keeps an id only while a connection asks: Kumi in Hermes never
# does, but a standalone Kumi connected to the same DAW still can (8b), and the id is then saved with the project.
_TAGGING_WHEN_ASKED = {
    "reaper": ("REAPER: Kumi's extension keeps an id in a saved project only when a standalone Kumi connected to REAPER "
               "asks it to; Kumi for Hermes never asks. Once kept, the id is saved with the project (ext state).",),
    "renoise": ("Renoise: Kumi's tool keeps an id in a song only when a standalone Kumi connected to Renoise asks it to; "
                "Kumi for Hermes never asks. Once kept, the id is saved with the song (its tool data).",),
}


def _tagging(daw: str, component: Mapping[str, Any]) -> tuple[str, ...]:
    """CC-8's line for `daw`: what its component keeps in a project. Only a real `keep_identity: true` (8c) says the
    component keeps an id only when asked, which a standalone Kumi beside Hermes still can (#1035, O-S4)."""
    return (_TAGGING_WHEN_ASKED if component.get("keep_identity") is True else _TAGGING).get(daw, ())


def plan(daw: str, *, setup_daw: Callable[[dict], Any] | None = None, data_dir: Path | None = None,
         plugin_dir: Path | None = None, target: str | None = None, current: str | None = None,
         use: bool = False) -> dict:
    """The next setup step and its exact effects, recorded in `<data>/setup/plan.json` for approval. No network.
    `setup_daw` calls the runtime's `setup_daw` tool; it is needed only once the runtime is installed. `current` is the
    plugin's DAW setting: the DAW step for another DAW also makes this one Kumi's DAW (`use_daw`, in the same approval)
    when `current`'s part isn't set up here, or when the producer asks (`use`). Raises only SetupError, including when
    the plan can't be written (`io_failed`)."""
    data_dir = data_dir or default_data_dir()
    result = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target or host_target(), current, use)
    result["made_at"] = _now_ms()
    result["digest"] = _digest(result)
    try:
        _write_json(data_dir / "setup" / "plan.json", result)
        component = result.get("component") or {}
        if result["step"] in ("none", "ffmpeg", "use_daw") and component.get("state") == "same":  # this home's
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
    """What the producer approves for `kumi_setup` `install` or `adopt`: `{"title", "effects", "message", "digest"}`
    (`message` is the title and one line per effect and per path written, each verbatim), from the recorded plan only
    (no network, no subprocess), or None when there is no fresh plan for `daw` that this action carries out, or one
    setup couldn't show as it is (the hook then blocks: "run kumi_setup plan first"). `install()` and `adopt()` carry
    out only the plan with that digest (their `expected_digest`), so what runs is what was approved, even if another
    chat plans meanwhile."""
    cached = _cached_plan(daw, data_dir or default_data_dir(), now)
    lines = _approval_lines(cached, daw, action) if cached else None
    if not lines or len(lines) > _MAX_LINES + 6 or not all(_plain(line, _MAX_LINE) for line in lines):
        return None  # the runtime's lines are capped at _MAX_LINES; setup adds at most a title and five of its own
    return {"title": lines[0], "effects": lines[1:], "message": "\n".join(lines), "digest": cached["digest"]}


def _approval_lines(plan: Mapping[str, Any], daw: str, action: str) -> list[str] | None:
    """The approval's title, then each effect of the plan and each path it writes, as the producer reads them, and
    whether it makes the DAW Kumi's DAW in this profile (`use_daw`)."""
    using = [plan["use_daw"]] if isinstance(plan.get("use_daw"), str) else []
    if action == "adopt":
        home = _owner_home(plan)
        if plan.get("step") != "owned_elsewhere" or home is None:
            return None
        component = plan["component"].get("component")
        kept = f"The {component} it installed" if component else f"Its part in {DAWS[daw]}"
        return [f"Kumi: use the existing Kumi installation at {home} for {DAWS[daw]}",
                "Kumi's runtime will read that installation's DAW connection files and write its own connection state "
                "there (never its sign-in, settings or account)",
                f"{kept} stays as it is", _RESTART, *_tagging(daw, plan["component"]), *using]
    if action != "install" or plan.get("step") not in ("runtime", "daw_component", "ffmpeg", "use_daw"):
        return None
    if plan["step"] == "use_daw" and not using:
        return None
    title = {"runtime": f"Kumi: download and install Kumi {plan['version']}",
             "daw_component": f"Kumi: install Kumi's part in {DAWS[daw]}",
             "ffmpeg": "Kumi: let Kumi's runtime run the ffmpeg already on this computer",
             "use_daw": f"Kumi: make {DAWS[daw]} Kumi's DAW in this profile"}[plan["step"]]
    return [title, *plan["effects"], *(f"Writes: {path}" for path in plan["paths"]), *using]


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
        claimed = _read_json(taken)
    finally:
        taken.unlink(missing_ok=True)
    if (not claimed or claimed.get("daw") != daw or claimed.get("digest") != expected_digest
            or claimed.get("digest") != _digest(claimed)):
        raise SetupError("plan_changed", "The setup plan changed after it was approved; plan again and approve that.")
    if not isinstance(claimed.get("made_at"), int) or _now_ms() - claimed["made_at"] > PLAN_TTL_MS:
        raise SetupError("expired", f"The approved setup plan is more than {PLAN_TTL_MS // 60000} minutes old, so "
                                    "setup didn't use it; plan again and approve that.")
    if _approval_lines(claimed, daw, action) is None:
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
          data_dir: Path | None = None, plugin_dir: Path | None = None, target: str | None = None,
          current: str | None = None, use: bool = False) -> dict:
    """Adopt the Kumi installation that owns this DAW's component, after the user approved `approval(daw,
    action="adopt")` (`expected_digest` is that approval's digest): from now on the runtime runs with `--kumi-home
    <that folder>`, so it uses the bridge that is already installed instead of replacing it. Re-plans first (with the
    `current` DAW setting and `use` the plan had) and refuses if anything changed. Setup itself never reads that
    installation's files, account or settings: it records the folder in `current.json` and the receipt. A runtime
    already running keeps its old home until W2 restarts it. Returns `{"ok": ...}` like `install()`; never raises."""
    data_dir = data_dir or default_data_dir()
    target = target or host_target()
    try:
        with _setup_lock(data_dir):
            approved = _claim(daw, data_dir, expected_digest, "adopt")
            replanned = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target, current, use)
            if _digest(replanned) != _digest(_unstamped(approved)):
                raise SetupError("plan_changed", "What setup found changed since it was approved; plan again.")
            home = _owner_home(approved)
            if not Path(home).is_dir():
                raise SetupError("not_found", f"{home} isn't a folder any more; plan again.")
            _adopt(approved, daw, home, data_dir)
        using = isinstance(approved.get("use_daw"), str)
        message = f"Kumi now uses the Kumi installation that already serves {DAWS[daw]}."
        if not using:
            message = f"{message} {next_daw(daw, current)}".strip()
        return {"ok": True, "step": "adopt", "message": message, **({"use_daw": daw} if using else {}),
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
            opener: Callable[..., Any] | None = None, current: str | None = None, use: bool = False) -> dict:
    """Do the step the user approved, and only that: the plan whose digest `approval()` gave (`expected_digest`),
    taken so it runs once, re-planned (with the `current` DAW setting and `use` the plan had) and refused if anything
    changed since, one setup step at a time. Returns `{"ok": True, "step", "status", "message"}`, with `"use_daw":
    daw` when the approved plan makes it Kumi's DAW (the caller sets the plugin's `daw` setting: setup has no say in
    Hermes's settings), or `{"ok": False, "error": {"code", "message"}}`; never raises."""
    data_dir = data_dir or default_data_dir()
    target = target or host_target()
    try:
        with _setup_lock(data_dir):
            approved = _claim(daw, data_dir, expected_digest, "install")
            replanned = _compute_plan(daw, setup_daw, data_dir, plugin_dir, target, current, use)
            if _digest(replanned) != _digest(_unstamped(approved)):
                raise SetupError("plan_changed", "What setup would do changed since it was approved; plan again.")
            if approved["step"] == "runtime":
                message = _install_runtime(approved, data_dir, target, progress, opener or _open_url)
            elif approved["step"] == "ffmpeg":
                message = _allow_ffmpeg(approved, data_dir)
            elif approved["step"] == "use_daw":
                message = ""
            else:
                message = _install_component(approved, setup_daw, data_dir)
                if not isinstance(approved.get("use_daw"), str):
                    message = f"{message} {next_daw(daw, current)}".strip()
        return {"ok": True, "step": approved["step"], "message": message,
                **({"use_daw": daw} if isinstance(approved.get("use_daw"), str) else {}),
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
        raise SetupError("download_failed", f"Downloading Kumi failed ({error}). Check your internet connection and run "
                                            "setup again; nothing was installed.") from None
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
        if isinstance(previous.get("tools"), dict):  # an ffmpeg the producer allowed stays allowed (CC-10)
            current["tools"] = previous["tools"]
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
    return f"Kumi {version}'s runtime is installed."


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


def _allow_ffmpeg(approved: Mapping[str, Any], data_dir: Path) -> str:
    """Record the ffmpeg the producer allowed, as the program it is now, where runtime.py reads it (CC-10)."""
    chosen = {key: approved["ffmpeg"][key] for key in _IDENTITY}
    if _resolved(chosen["path"]) != chosen["path"] or _identity(chosen["path"]) != chosen:
        raise SetupError("plan_changed", f"{chosen['path']} changed since it was approved; plan again.")
    record = _read_json(data_dir / "runtime" / "current.json")
    if not record:
        raise SetupError("needs_runtime", "Install Kumi's runtime before allowing ffmpeg.")
    tools = record.get("tools") if isinstance(record.get("tools"), dict) else {}
    _write_json(data_dir / "runtime" / "current.json", {**record, "tools": {**tools, "ffmpeg": chosen}})
    receipt = _read_json(data_dir / "setup" / "receipt.json") or {"schema": SCHEMA}
    receipt["ffmpeg"] = {**chosen, "at": _now_ms()}
    _write_json(data_dir / "setup" / "receipt.json", receipt)
    return "Kumi's runtime will use the ffmpeg you allowed for compressed audio from its next start."


def _install_component(approved: dict, setup_daw: Callable[[dict], Any] | None, data_dir: Path) -> str:
    if setup_daw is None:
        raise SetupError("needs_runtime_client", "Setup needs Kumi's runtime for the DAW step.")
    # The runtime installs only the plan that was approved (C8): it plans again under the DAW's lease, and refuses
    # (`changed`) when that plan's digest isn't the approved one's.
    args = {"daw": approved["daw"], "dry_run": False}
    if (approved.get("component") or {}).get("digest"):
        args["expected"] = approved["component"]["digest"]
    done = _ask_runtime(setup_daw, args, f"Installing Kumi's part in {DAWS[approved['daw']]} failed")
    _record_component(data_dir, approved["daw"], done, installed_at=done.get("installed_at") or _now_ms())
    after = {"choose_control_surface": " In Live, choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI).",
             "restart_daw": f" Restart {DAWS[approved['daw']]} to load it.",
             "enable_tool": " Enable the Kumi tool in Renoise's Tool Browser."}.get(done.get("after") or "", "")
    return f"Kumi's part in {DAWS[approved['daw']]} is installed.{after}"


# ---- `hermes kumi setup` ---------------------------------------------------------------------------------------------

def cli_main(daw: str | None = None, *, setup_daw: Callable[[dict], Any] | None = None,
             data_dir: Path | None = None, plugin_dir: Path | None = None, target: str | None = None,
             opener: Callable[..., Any] | None = None, stdin: Any = None, stdout: Any = None,
             current: str | None = None, use: bool = False, make_current: Callable[[str], None] | None = None) -> int:
    """`hermes kumi setup`: show each step, ask before it, refuse without a terminal. There is no --yes. `current` is
    the plugin's DAW setting (else read from Hermes's config); when a step the producer approved makes `daw` Kumi's
    DAW (`plan`'s `use_daw`; `use` asks for it even when `current` is set up), `make_current(daw)` sets it."""
    stdin, stdout = stdin or sys.stdin, stdout or sys.stdout
    if not (stdin.isatty() and stdout.isatty()):
        print("hermes kumi setup asks before each step, so it needs a terminal; in a chat, ask Hermes to set up Kumi.",
              file=stdout)
        return 1
    current = current if current in DAWS else _configured_daw()
    daw = daw if daw in DAWS else current
    for _ in range(5):  # the runtime, the DAW component, making it Kumi's DAW, ffmpeg if there is one, a final check
        try:
            step = plan(daw, setup_daw=setup_daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target,
                        current=current, use=use)
        except SetupError as error:
            print(error.message, file=stdout)
            return 1
        if step["step"] == "none":
            now = status(daw=daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)
            print(now["message"], now["compressed_audio"]["message"], *filter(None, [next_daw(daw, current)]),
                  sep="\n", file=stdout)
            return 0
        adopting = step["step"] == "owned_elsewhere"
        lines = _approval_lines(step, daw, "adopt" if adopting else "install")  # what a chat's approval shows
        if lines is None:  # another installation owns the component, and it names no folder to adopt
            print("Another Kumi installation owns this DAW's component:", *step["effects"], sep="\n  ", file=stdout)
            return 1
        print(lines[0], *lines[1:], sep="\n  ", file=stdout)
        print("Go ahead? [y/N] ", end="", file=stdout, flush=True)
        if stdin.readline().strip().lower() not in ("y", "yes"):
            if step.get("optional"):  # setup is done without it
                print(status(daw=daw, data_dir=data_dir, plugin_dir=plugin_dir, target=target)["message"],
                      "Kumi reads WAV and AIFF only; run hermes kumi setup again to allow ffmpeg later.",
                      sep="\n", file=stdout)
                return 0
            print("Nothing was changed.", file=stdout)
            return 1
        if adopting:
            result = adopt(daw, expected_digest=step["digest"], setup_daw=setup_daw, data_dir=data_dir,
                           plugin_dir=plugin_dir, target=target, current=current, use=use)
        else:
            result = install(daw, expected_digest=step["digest"], setup_daw=setup_daw, data_dir=data_dir,
                             plugin_dir=plugin_dir, target=target, opener=opener, current=current, use=use)
        if not result["ok"]:
            print(result["error"]["message"], file=stdout)
            return 1
        message = result["message"]
        if result.get("use_daw") == daw:
            try:
                if make_current is None:
                    raise RuntimeError("no settings writer")
                make_current(daw)
            except Exception:
                print(f"{message} Kumi couldn't change its DAW setting, so it still connects to {DAWS[current]}: set "
                      f"it to {daw} in Hermes's settings for the Kumi plugin.".strip(), file=stdout)
                return 1
            current = daw
            message = f"{message} {DAWS[daw]} is Kumi's DAW in this profile now.".strip()
        print(message, file=stdout)
        if adopting:
            return 0
        if step["step"] == "runtime" and setup_daw is None:
            return 0
    return 0
