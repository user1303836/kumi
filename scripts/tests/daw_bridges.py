#!/usr/bin/env python3
"""REAPER's extension and Renoise's tool as Kumi puts them in the DAWs' folders, and Logic Pro's bridge as Kumi sets it
up in its own, for the Installer workflow's jobs.

    python3 scripts/tests/daw_bridges.py prepare <reaper-resource> <renoise-version-folder>
    python3 scripts/tests/daw_bridges.py placed <app> <kumi-home> <reaper-resource> <renoise-version-folder> [--load] [--kept-logic]
    python3 scripts/tests/daw_bridges.py removed <kumi-home> <reaper-resource> <renoise-version-folder>
    python3 scripts/tests/daw_bridges.py earlier <app> <version>

`prepare` makes the DAWs' folders as each makes them when it first runs (REAPER's resource folder with its reaper.ini,
Renoise's version folder), each with a file of the producer's own (`theirs`) where Kumi puts its bridge. `placed` checks
each DAW holds exactly the copy the Kumi in <app> carries, Kumi's reference beside it and a receipt
naming its version, with the generation it came from kept in Kumi's folder; it prints one line per DAW (version,
SHA-256, generation, the receipt's last action) to compare across updates and rollbacks. --load also loads the
extension with this system's loader and finds REAPER's entry point, and starts Logic's bridge to ask its version.
`removed` checks each is gone, with its reference and receipt. Both check the producer's own files beside them
(`theirs`) are as they were.

Logic's bridge has no DAW folder: Kumi sets it up in <kumi-home>/bridges/logic/bridge/logic-kumi, from the
daws/logic/logic-kumi folder the Kumi in <app> carries, with the version file in it. Only the Apple Silicon Mac's Kumi
carries it (Logic Pro 12 runs on Apple Silicon only), so `placed` checks it there, and elsewhere (Linux, Windows, an
Intel Mac) that Kumi carries none and set none up. With --kept-logic, a Logic Pro bridge a later Kumi set up may stay
where it is (a rollback to a Kumi that carries none), and is listed as it stands.

`earlier` makes the Kumi in <app> carry other copies of each, as an earlier release with other versions of them
would: no release before this one carries them yet.
"""
from __future__ import annotations

import ctypes
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys

EXTENSION = "reaper_kumi" + {"darwin": ".dylib", "win32": ".dll"}.get(sys.platform, ".so")
TOOL = "com.kumi.Bridge.xrnx"
# What Kumi writes into the tool's folder beside its files, and Renoise its preferences: not the tool's own.
NOT_PAYLOAD = ("bridge-reference.json", "preferences.xml")
THEIRS = b"the producer's own\n"
# Logic's bridge, a folder (its reference inside it, as the tool's is), and the one bundle that carries it.
LOGIC = "logic-kumi"
LOGIC_TARGET = "aarch64-apple-darwin"


def fail(message: str) -> None:
    raise SystemExit(f"daw_bridges.py: {message}")


def tree(path: Path) -> dict[str, bytes]:
    """A file's bytes, or a folder's files by relative path (less what isn't the tool's own, at its top)."""
    if path.is_file():
        return {"": path.read_bytes()}
    return {file.relative_to(path).as_posix(): file.read_bytes() for file in sorted(path.rglob("*"))
            if file.is_file() and file.relative_to(path).as_posix() not in NOT_PAYLOAD}


def sha(files: dict[str, bytes]) -> str:
    """daw-bridge's tree_sha256, which the receipt records: a file's SHA-256, or one over a folder's sorted rows."""
    if list(files) == [""]:
        return hashlib.sha256(files[""]).hexdigest()
    rows = sorted(f"{name}\0{len(data)}\0{hashlib.sha256(data).hexdigest()}\n" for name, data in files.items())
    return hashlib.sha256("".join(rows).encode()).hexdigest()


def daws(reaper: Path, renoise: Path) -> dict[str, tuple[Path, Path, Path]]:
    """Each DAW's installed copy, Kumi's reference for it, and the producer's own file beside it."""
    plugins, tools = reaper / "UserPlugins", renoise / "Scripts" / "Tools"
    return {"reaper": (plugins / EXTENSION, plugins / "kumi-bridge-reference.json", plugins / ("reaper_theirs" + Path(EXTENSION).suffix)),
            "renoise": (tools / TOOL, tools / TOOL / "bridge-reference.json", tools / "com.someone.Tool.xrnx" / "main.lua")}


def logic(home: Path) -> tuple[Path, Path, None]:
    """Logic's bridge as Kumi sets it up: in Kumi's own folder, with Kumi's reference inside it, and no file of the
    producer's beside it (Logic loads nothing, so there's no DAW folder to share)."""
    folder = home / "bridges" / "logic" / "bridge" / LOGIC
    return folder, folder / "bridge-reference.json", None


def carries_logic(app: Path) -> bool:
    """Whether the Kumi in `app` is the Apple Silicon Mac's (as its kumi-install.json says), the one that carries Logic's
    bridge; refused when what it carries says otherwise."""
    target = json.loads((app / "kumi-install.json").read_text(encoding="utf-8"))["target"]
    carries = (app / "daws" / "logic" / LOGIC).exists()
    if carries != (target == LOGIC_TARGET):
        fail(f"the Kumi in {app} is for {target} and carries {'a' if carries else 'no'} Logic Pro bridge: only {LOGIC_TARGET}'s carries one")
    return carries


def carried(app: Path) -> dict[str, tuple[Path, str]]:
    """What the Kumi in `app` carries for each DAW, and its version."""
    reaper = app / "daws" / "reaper"
    tool = app / "daws" / "renoise" / TOOL
    lua = (tool / "payload" / "version.lua").read_text(encoding="utf-8")
    bridges = {"reaper": (reaper / EXTENSION, (reaper / "reaper_kumi.version").read_text(encoding="ascii").strip()),
               "renoise": (tool, lua.split('"')[1])}
    if carries_logic(app):
        bridge = app / "daws" / "logic" / LOGIC
        bridges["logic"] = (bridge, (bridge / "version").read_text(encoding="ascii").strip())
    return bridges


def receipt(home: Path, daw: str) -> dict:
    path = home / "bridges" / daw / "install-receipt.json"
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}


def load(extension: Path) -> None:
    """The extension through this system's loader, and REAPER's entry point in it: what REAPER does first. Only where
    this Python's architecture is one the file holds (an Arm Windows Python can't load the x64 build REAPER takes)."""
    machine = platform.machine().lower()
    if sys.platform == "win32" and machine in ("arm64", "aarch64"):
        print(f"(not loaded: an {machine} Python can't load the x64 extension)")
        return
    library = ctypes.WinDLL(str(extension)) if sys.platform == "win32" else ctypes.CDLL(str(extension))
    if not hasattr(library, "ReaperPluginEntry"):
        fail(f"{extension} has no ReaperPluginEntry")


def start(bridge: Path, version: str) -> None:
    """Logic's bridge as Kumi set it up, started as the host starts it and asked only its version: macOS on Apple Silicon
    runs it only signed, and laid out as dyld wants."""
    said = subprocess.run([str(bridge / LOGIC), "--version"], capture_output=True, text=True, timeout=60)
    if said.returncode != 0 or said.stdout.strip() != f"logic-kumi {version}":
        fail(f"{bridge / LOGIC} --version exited {said.returncode} saying {(said.stdout + said.stderr).strip()!r}, not logic-kumi {version}")


def placed(app: Path, home: Path, reaper: Path, renoise: Path, loading: bool, kept_logic: bool = False) -> None:
    lines = []
    bridges = carried(app)
    places = {**daws(reaper, renoise), "logic": logic(home)}
    for daw, (source, version) in bridges.items():
        target, reference, theirs = places[daw]
        if not target.exists():
            fail(f"{target} isn't there")
        if tree(target) != tree(source):
            fail(f"{target} isn't the copy {source} carries")
        said = json.loads(reference.read_text(encoding="utf-8"))
        if said.get("daw") != daw or not Path(said.get("secretFile", "")).is_file():
            fail(f"{reference} isn't Kumi's reference for {daw}, with its secret: {said}")
        if theirs is not None and theirs.read_bytes() != THEIRS:
            fail(f"{theirs}, the producer's own, was changed")
        kept = receipt(home, daw)
        if (kept.get("status"), kept.get("version"), kept.get("target")) != ("installed", version, str(target)):
            fail(f"{daw}'s receipt says {kept.get('status')} {kept.get('version')} at {kept.get('target')}, not installed {version} at {target}")
        if kept.get("sha256") != sha(tree(target)):
            fail(f"{daw}'s receipt records {kept.get('sha256')}, and {target} is {sha(tree(target))}")
        generation = home / "bridges" / daw / "generations" / kept["generation"] / target.name
        if tree(generation) != tree(source):
            fail(f"{daw}'s generation {kept['generation']} isn't the copy in place")
        lines.append(f"{daw} {version} {sha(tree(target))[:12]} {kept['generation']} {kept['history'][0]['action']}")
    if "logic" not in bridges and (logic(home)[0].exists() or receipt(home, "logic").get("status") == "installed"):
        kept = receipt(home, "logic")
        if not (kept_logic and logic(home)[0].exists() and kept.get("status") == "installed"):
            fail(f"{logic(home)[0]} was set up by a Kumi that carries no Logic Pro bridge: only {LOGIC_TARGET}'s carries one")
        # (--kept-logic: a rollback to a Kumi that carries none leaves the bridge a later one set up where it is)
        lines.append(f"logic {kept.get('version')} {sha(tree(logic(home)[0]))[:12]} {kept['generation']} {kept['history'][0]['action']}")
    if loading:
        load(daws(reaper, renoise)["reaper"][0])
        if "logic" in bridges:
            start(logic(home)[0], bridges["logic"][1])
    print("\n".join(lines))


def prepare(reaper: Path, renoise: Path) -> None:
    (reaper / "UserPlugins").mkdir(parents=True)
    (reaper / "reaper.ini").write_bytes(b"[REAPER]\n")
    for _, _, theirs in daws(reaper, renoise).values():
        theirs.parent.mkdir(parents=True, exist_ok=True)
        theirs.write_bytes(THEIRS)


def removed(home: Path, reaper: Path, renoise: Path) -> None:
    for daw, (target, reference, theirs) in {**daws(reaper, renoise), "logic": logic(home)}.items():
        for path in (target, reference):
            if path.exists():
                fail(f"{path} is still there")
        if theirs is not None and theirs.read_bytes() != THEIRS:
            fail(f"{theirs}, the producer's own, was changed or removed")
        if receipt(home, daw).get("status") == "installed":
            fail(f"{daw}'s receipt still says installed")
    print("each removed; the producer's own files stayed")


def earlier(app: Path, version: str) -> None:
    """Other copies of each in `app`, versioned `version`: a byte more in the extension (a copy Kumi never loads), and
    the tool's version file and Logic's bridge's (where the Kumi in `app` carries one) saying so."""
    extension = app / "daws" / "reaper" / EXTENSION
    extension.write_bytes(extension.read_bytes() + b"\0")
    (app / "daws" / "reaper" / "reaper_kumi.version").write_text(version + "\n", encoding="ascii")
    lua = app / "daws" / "renoise" / TOOL / "payload" / "version.lua"
    lua.write_text(f'-- The bridge\'s own version, reported in hello and the discovery file.\nreturn "{version}"\n', encoding="utf-8")
    if carries_logic(app):
        (app / "daws" / "logic" / LOGIC / "version").write_text(version + "\n", encoding="ascii")


def main() -> None:
    args = sys.argv[1:]
    loading, kept_logic = "--load" in args, "--kept-logic" in args
    args = [arg for arg in args if arg not in ("--load", "--kept-logic")]
    if args[:1] == ["prepare"] and len(args) == 3:
        prepare(Path(args[1]), Path(args[2]))
    elif args[:1] == ["placed"] and len(args) == 5:
        placed(*map(Path, args[1:]), loading, kept_logic)
    elif args[:1] == ["removed"] and len(args) == 4:
        removed(*map(Path, args[1:]))
    elif args[:1] == ["earlier"] and len(args) == 3:
        earlier(Path(args[1]), args[2])
    else:
        fail(__doc__.strip().splitlines()[0])


if __name__ == "__main__":
    main()
