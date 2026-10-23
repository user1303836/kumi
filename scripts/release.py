#!/usr/bin/env python3
"""Cut a Kumi release from main in one command (Python 3.11+, git, and gh signed in as a repository admin).

    python3 scripts/release.py          # a dry run: the edits and checks in a scratch worktree, which is removed
    python3 scripts/release.py --go     # the release

With --go it commits "Kumi X.Y.Z: the changelog, READMEs and versions" on release/vX.Y.Z, opens the pull request
"Kumi X.Y.Z" and merges it with the admin bypass as "Kumi X.Y.Z (#PR)" without waiting for its CI (the tag build
checks the same commit on six platforms, and main's CI runs on it too). Then it tags the merge commit vX.Y.Z,
pushes the tag and creates the draft release with its notes. The tag's Installer run attaches the bundles and
publishes the release once every install and migration job has passed.

What ships is read from main since the last release tag:
- Each pull request's "Changelog:" lines, verbatim. "none" adds nothing. A line goes under the bridge's heading
  when its pull request changes the bridge and nothing of Kumi's (crates/kumi*, install.sh, install.ps1), or
  when it's written "Changelog (bridge):"; "Changelog (kumi):" keeps it under Kumi's.
- The bridge gets a new version when a pull request changes what Live loads: the bridge host, the code it shares
  with Kumi (kumi-common), the Remote Script, the protocol, the Live extension or Willington's files (their tests
  and Markdown aside). A dependency change can change the host too: the dry run says so, and --bridge gives the
  bridge a new version then. --no-bridge keeps it.
- REAPER's extension and Renoise's tool get new versions the same way, each when a pull request changes what its DAW
  loads: the extension (daws/reaper/reaper-kumi, and daw-bridge, which it links) or the tool (daws/renoise's
  com.kumi.Bridge.xrnx). --reaper and --renoise give one a new version whatever changed; --no-reaper and --no-renoise
  keep it. A pull request that changes one of them and nothing else that ships has its lines under that one's
  heading, as do lines written "Changelog (reaper):" or "Changelog (renoise):". --no-bridge is only Live's.
- A pull request's files count where they are now: one from before a move (scripts/moves.toml) by their new places,
  and an older release's versions are read where it had them.
The version goes up by a patch, by a minor with --minor, or is given with --version.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import datetime
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import tomllib

ROOT = Path(__file__).resolve().parents[1]
# The script that builds the bundles, one copy shared with its tests: where the DAWs' bridges are, and how Renoise's
# manifest numbers a version.
native = sys.modules.get("native_release")
if native is None:
    SPEC = importlib.util.spec_from_file_location("native_release", ROOT / "scripts/build-native-release.py")
    native = importlib.util.module_from_spec(SPEC)
    sys.modules["native_release"] = native
    SPEC.loader.exec_module(native)
# A change under these, besides tests and Markdown, is a new bridge for Live to load. The host links kumi-common.
BRIDGE = ("daws/ableton/ableton-mcp-server/", "crates/kumi-common/", "daws/ableton/remote-script/", "daws/ableton/protocol/",
          "daws/ableton/live-extension/", "vendor/willington/")
# The same for REAPER's extension, and the parts of daw-bridge it links (its own wire, handshake, auth, discovery and
# files: `rg daw_bridge:: daws/reaper/reaper-kumi/src`, and what those use), and for Renoise's tool (their tests and examples
# aside too). The rest of daw-bridge (the lifecycle, detection, the doctor, installing) is Kumi's alone.
REAPER = (native.REAPER_CRATE + "/", "daws/own/daw-bridge/Cargo.toml", *(f"daws/own/daw-bridge/src/{name}.rs" for name in (
    "lib", "auth", "contract", "daw", "files", "frame", "handshake", "loopback", "owner", "paths", "wire")))
RENOISE = (native.RENOISE_TOOL_SOURCE + "/",)
REAPER_CARGO = native.REAPER_CRATE + "/Cargo.toml"
RENOISE_VERSION = native.RENOISE_TOOL_SOURCE + "/payload/version.lua"
RENOISE_MANIFEST = native.RENOISE_TOOL_SOURCE + "/manifest.xml"
# Kumi's own, daw-bridge and its host for REAPER and Renoise (daw-mcp-server) among them.
KUMI = ("crates/kumi/", "crates/kumi-runtime/", "crates/kumi-types/", "crates/kumi-net/", "crates/kumi-media/", "crates/kumi-ai/",
        "crates/kumi-web/", "crates/kumi-common/", "crates/kumi-store/",
        "daws/own/daw-bridge/", "daws/own/daw-mcp-server/", "install.sh", "install.ps1")
# Kumi's own crates, at Kumi's version, and their folders: Kumi, its runtime, common parts and shared types, and its host
# for REAPER and Renoise.
CRATES = {"kumi": "crates/kumi", "kumi-common": "crates/kumi-common", "kumi-runtime": "crates/kumi-runtime",
          "kumi-types": "crates/kumi-types", "daw-mcp-server": "daws/own/daw-mcp-server"}
# Where paths moved in the repository, oldest first (scripts/moves.toml, which `dev.py rebase-moves` reads too).
MOVES = [(move["from"], move["to"]) for move in
         tomllib.loads((ROOT / "scripts/moves.toml").read_text(encoding="utf-8")).get("move", [])]
# The CHANGELOG's dates are New York's, as its earlier entries are.
TIMEZONE = "America/New_York"
WIDTH = 102
LINE = re.compile(r"^Changelog(?:\s*\((?P<where>kumi|bridge|reaper|renoise)\))?:\s*(?P<text>.+?)\s*$", re.M | re.I)
# The notes end with this, unrendered: the tag build publishes only a draft whose notes carry it.
MARKER = "<!-- kumi:release-notes -->"
NUMBER = r"\d+\.\d+(?:\.\d+)?"  # the oldest ships-with entries say "1.5"
# The ships-with line in each KUMI_CHANGES.md: the pattern of its first entry, that entry with a longer range, the
# range's dash, and a new first entry in front of the old one.
SHIPS = {
    "en": (rf"Kumi (?P<first>{NUMBER})(?: to (?P<last>{NUMBER}))? with bridge (?P<bridge>{NUMBER}), ",
           "Kumi {range} with bridge {bridge}, ", " to ", "Kumi {new} with bridge {new_bridge}, {range} with {bridge}, "),
    "ja": (rf"Kumi (?P<first>{NUMBER})(?:〜(?P<last>{NUMBER}))? にはブリッジ (?P<bridge>{NUMBER})、",
           "Kumi {range} にはブリッジ {bridge}、", "〜", "Kumi {new} にはブリッジ {new_bridge}、{range} には {bridge}、"),
    "zh-CN": (rf"Kumi (?P<first>{NUMBER})(?: 至 (?P<last>{NUMBER}))? 附带桥接 (?P<bridge>{NUMBER})，",
              "Kumi {range} 附带桥接 {bridge}，", " 至 ", "Kumi {new} 附带桥接 {new_bridge}，{range} 附带 {bridge}，"),
}
RESTART = ("Ships with bridge {bridge}, which Live loads when it restarts. After updating, Kumi offers to quit Live "
           "(it asks to save first) and opens it again with the new bridge; or run `kumi bridge` with Live closed.")
# REAPER's extension and Renoise's tool: what the CHANGELOG, the notes and the docs call each, and its DAW.
DAWS = {"reaper": ("REAPER extension", "REAPER"), "renoise": ("Renoise tool", "Renoise")}
# Each one's running ships-with line in the docs, in any docs/<language>/*.md (one in every language, or none in any):
# the pattern of its first entry, that entry with a longer range, the range's dash, and a new first entry in front
# of the old one. A first entry that ends the sentence (the first release of it) still ends it, after the old one.
DAW_SHIPS = {
    "reaper": {
        "en": (rf"Kumi (?P<first>{NUMBER})(?: to (?P<last>{NUMBER}))? with REAPER extension (?P<bridge>{NUMBER})(?P<end>, |\.)",
               "Kumi {range} with REAPER extension {bridge}{end}", " to ",
               "Kumi {new} with REAPER extension {new_bridge}, {range} with {bridge}{end}"),
        "ja": (rf"Kumi (?P<first>{NUMBER})(?:〜(?P<last>{NUMBER}))? には REAPER 拡張機能 (?P<bridge>{NUMBER})(?P<end>、|。)",
               "Kumi {range} には REAPER 拡張機能 {bridge}{end}", "〜",
               "Kumi {new} には REAPER 拡張機能 {new_bridge}、{range} には {bridge}{end}"),
        "zh-CN": (rf"Kumi (?P<first>{NUMBER})(?: 至 (?P<last>{NUMBER}))? 附带 REAPER 扩展 (?P<bridge>{NUMBER})(?P<end>，|。)",
                  "Kumi {range} 附带 REAPER 扩展 {bridge}{end}", " 至 ",
                  "Kumi {new} 附带 REAPER 扩展 {new_bridge}，{range} 附带 {bridge}{end}"),
    },
    "renoise": {
        "en": (rf"Kumi (?P<first>{NUMBER})(?: to (?P<last>{NUMBER}))? with Renoise tool (?P<bridge>{NUMBER})(?P<end>, |\.)",
               "Kumi {range} with Renoise tool {bridge}{end}", " to ",
               "Kumi {new} with Renoise tool {new_bridge}, {range} with {bridge}{end}"),
        "ja": (rf"Kumi (?P<first>{NUMBER})(?:〜(?P<last>{NUMBER}))? には Renoise ツール (?P<bridge>{NUMBER})(?P<end>、|。)",
               "Kumi {range} には Renoise ツール {bridge}{end}", "〜",
               "Kumi {new} には Renoise ツール {new_bridge}、{range} には {bridge}{end}"),
        "zh-CN": (rf"Kumi (?P<first>{NUMBER})(?: 至 (?P<last>{NUMBER}))? 附带 Renoise 工具 (?P<bridge>{NUMBER})(?P<end>，|。)",
                  "Kumi {range} 附带 Renoise 工具 {bridge}{end}", " 至 ",
                  "Kumi {new} 附带 Renoise 工具 {new_bridge}，{range} 附带 {bridge}{end}"),
    },
}


class Refused(Exception):
    """A release that can't go ahead as things are; the message says why."""


@dataclass
class Change:
    number: int
    paths: list[str]
    lines: list[tuple[str, str]]  # (kumi | bridge | reaper | renoise, text)
    written: bool  # the body has a Changelog line, "none" included
    wrapped: list[str]  # Changelog lines whose sentence seems to go on past the line break


@dataclass
class Plan:
    old: str
    new: str
    bridge: str
    new_bridge: str
    date: str
    kumi: list[str]
    bridges: list[str]
    # REAPER's extension and Renoise's tool, their versions before and after and their own lines (None: this main has
    # no such bridge).
    reaper: str | None = None
    new_reaper: str | None = None
    renoise: str | None = None
    new_renoise: str | None = None
    reaper_lines: list[str] = field(default_factory=list)
    renoise_lines: list[str] = field(default_factory=list)
    # The DAWs whose bridges this release brings (the last release had none): each ships at the version it has.
    brought: list[str] = field(default_factory=list)

    @property
    def same_bridge(self) -> bool:
        return self.new_bridge == self.bridge

    @property
    def same_reaper(self) -> bool:
        return self.new_reaper == self.reaper

    @property
    def same_renoise(self) -> bool:
        return self.new_renoise == self.renoise

    def daw(self, name: str) -> tuple[str | None, str | None, list[str]]:
        """A DAW's bridge's version before and after, and its lines."""
        if name == "reaper":
            return self.reaper, self.new_reaper, self.reaper_lines
        return self.renoise, self.new_renoise, self.renoise_lines

    def moved(self) -> list[str]:
        """The DAWs whose bridges get a new version, REAPER's first."""
        return [name for name in DAWS if self.daw(name)[0] != self.daw(name)[1] and name not in self.brought]

    def olds(self) -> list[str]:
        """The versions this release moves on from: Kumi's, and each bridge's that changes."""
        return [self.old] + [old for old, new in [(self.bridge, self.new_bridge), *(self.daw(name)[:2] for name in DAWS)]
                             if old is not None and old != new]


def run(args: list[str], cwd: Path = ROOT) -> str:
    result = subprocess.run(args, cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        raise Refused(f"{' '.join(args)} failed ({result.returncode}):\n{result.stdout}{result.stderr}".rstrip())
    return result.stdout.strip()


def bump(version: str, minor: bool = False) -> str:
    major, middle, patch = map(int, version.split("."))
    return f"{major}.{middle + 1}.0" if minor else f"{major}.{middle}.{patch + 1}"


def newer(a: str, b: str) -> bool:
    return tuple(map(int, a.split("."))) > tuple(map(int, b.split(".")))


def today() -> str:
    try:
        from zoneinfo import ZoneInfo
        return datetime.datetime.now(ZoneInfo(TIMEZONE)).date().isoformat()
    except Exception:  # no time zone database: the local date
        return datetime.date.today().isoformat()


def now(path: str) -> str:
    """Where a path that a commit from before a move names is now."""
    for old, new in MOVES:
        if path == old or path.startswith(old + "/"):
            path = new + path[len(old):]
    return path


def then(path: str) -> list[str]:
    """`path`, then what it was before each move, newest first: an older commit has it at one of them."""
    names = [path]
    for old, new in reversed(MOVES):
        if names[-1] == new or names[-1].startswith(new + "/"):
            names.append(old + names[-1][len(new):])
    return names


def ships_bridge(path: str) -> bool:
    name = path.rsplit("/", 1)[-1]
    return path.startswith(BRIDGE) and not path.endswith(".md") and "/tests/" not in path and not name.startswith("test_")


def ships_daw(path: str, under: tuple[str, ...]) -> bool:
    """A change under these that the DAW loads: not a test, an example or Markdown."""
    name = path.rsplit("/", 1)[-1]
    return (path.startswith(under) and not path.endswith(".md") and "/tests/" not in path and "/examples/" not in path
            and name != "tests.rs" and not name.startswith("test_"))


def ships_reaper(path: str) -> bool:
    return ships_daw(path, REAPER)


def ships_renoise(path: str) -> bool:
    return ships_daw(path, RENOISE)


def classify(number: int, body: str, paths: list[str]) -> Change:
    rows = (body or "").replace("\r\n", "\n").split("\n")
    found = [(i, m) for i, row in enumerate(rows) if (m := LINE.match(row))]
    kumis = any(path.startswith(KUMI) for path in paths)
    bridge, reaper, renoise = (any(ships(path) for path in paths) for ships in (ships_bridge, ships_reaper, ships_renoise))
    # A pull request whose lines are a bridge's alone: it changes that bridge and nothing else that ships.
    alone = ("bridge" if bridge and not kumis else "reaper" if reaper and not (kumis or bridge or renoise)
             else "renoise" if renoise and not (kumis or bridge or reaper) else "kumi")
    lines = [((m["where"] or alone).lower(), m["text"]) for _, m in found if not re.match(r"none\b", m["text"], re.I)]
    # Only a Changelog line's own row is read, so a sentence the body wraps onto the next row would be cut.
    wrapped = [m["text"] for i, m in found if i + 1 < len(rows) and rows[i + 1].strip()
               and not re.match(r"\s*(Changelog\b|[-*>#|`<🤖]|\d+\.\s)", rows[i + 1], re.I)]
    return Change(number, paths, lines, bool(found), wrapped)


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    if text.count(old) != 1:
        raise Refused(f"{path.name}: expected one {old!r}, found {text.count(old)}")
    path.write_text(text.replace(old, new), encoding="utf-8")


def ships_with(text: str, language: str, plan: Plan) -> str:
    pattern, same, dash, moved = SHIPS[language]
    found = list(re.finditer(pattern, text))
    if len(found) != 1:
        raise Refused(f"KUMI_CHANGES.md ({language}): expected one ships-with line, found {len(found)}")
    entry = found[0]
    if (entry["last"] or entry["first"]) != plan.old or entry["bridge"] != plan.bridge:
        raise Refused(f"KUMI_CHANGES.md ({language}) starts {entry[0]!r}, not Kumi {plan.old} with bridge {plan.bridge}")
    if plan.same_bridge:
        line = same.format(range=f"{entry['first']}{dash}{plan.new}", bridge=plan.bridge)
    else:
        first = entry["first"] + (f"{dash}{entry['last']}" if entry["last"] else "")
        line = moved.format(new=plan.new, new_bridge=plan.new_bridge, range=first, bridge=plan.bridge)
    return text[:entry.start()] + line + text[entry.end():]


def daw_lines(root: Path, daw: str) -> dict[str, Path]:
    """The doc holding each language's running ships-with line for a DAW's bridge: one in every language, or none
    while the docs have none yet."""
    name = DAWS[daw][0]
    found: dict[str, Path] = {}
    for language, (pattern, *_) in DAW_SHIPS[daw].items():
        docs = sorted((root / "docs" / language).glob("*.md"))
        hits = [path for path in docs for _ in re.finditer(pattern, path.read_text(encoding="utf-8"))]
        if len(hits) > 1:
            raise Refused(f"docs/{language}: expected one {name} ships-with line, found {len(hits)} ({', '.join(p.name for p in hits)})")
        if hits:
            found[language] = hits[0]
    missing = [language for language in DAW_SHIPS[daw] if language not in found]
    if found and missing:
        raise Refused(f"the {name} ships-with line is in {', '.join(f'docs/{language}' for language in found)} but not in "
                      f"{', '.join(f'docs/{language}' for language in missing)}")
    return found


def daw_ships_with(text: str, daw: str, language: str, plan: Plan, where: str) -> str:
    """`text` (the doc at `where`) with a DAW's bridge's running ships-with line moved on to this release. One the
    release brings starts with this release already, and stays as it is."""
    pattern, same, dash, moved = DAW_SHIPS[daw][language]
    name = DAWS[daw][0]
    old, new, _ = plan.daw(daw)
    found = list(re.finditer(pattern, text))
    if len(found) != 1:
        raise Refused(f"{where}: expected one {name} ships-with line, found {len(found)}")
    entry = found[0]
    if daw in plan.brought:
        if entry["first"] != plan.new or entry["last"] or entry["bridge"] != new:
            raise Refused(f"{where}'s {name} line starts {entry[0]!r}: the first release of it says the release that brings "
                          f"it, Kumi {plan.new} with {name} {new}")
        return text
    if (entry["last"] or entry["first"]) != plan.old or entry["bridge"] != old:
        raise Refused(f"{where}'s {name} line starts {entry[0]!r}, not Kumi {plan.old} with {name} {old}")
    if old == new:
        line = same.format(range=f"{entry['first']}{dash}{plan.new}", bridge=old, end=entry["end"])
    else:
        first = entry["first"] + (f"{dash}{entry['last']}" if entry["last"] else "")
        line = moved.format(new=plan.new, new_bridge=new, range=first, bridge=old, end=entry["end"])
    return text[:entry.start()] + line + text[entry.end():]


def daws_said(plan: Plan, bold: bool = False) -> str:
    """What the CHANGELOG and the notes say of REAPER's extension and Renoise's tool: the ones this release changes,
    which their DAWs load when they restart, and the ones it brings; nothing of the rest."""
    shown = (lambda version: f"**{version}**") if bold else (lambda version: version)
    said = []
    for names, sentence in [(plan.moved(), "Ships with {bridges}, which {daws} {load} when {they} {restart}; `kumi update` puts {it} in."),
                            (plan.brought, "Brings {bridges}: {commands} {put} {it} in.")]:
        if names:
            many = len(names) > 1
            said.append(sentence.format(
                bridges=" and ".join(f"{DAWS[name][0]} {shown(plan.daw(name)[1])}" for name in names),
                daws=" and ".join(DAWS[name][1] for name in names), load="load" if many else "loads",
                they="they" if many else "it", restart="restart" if many else "restarts", it="them" if many else "it",
                commands=" and ".join(f"`kumi --daw {name} bridge`" for name in names), put="put" if many else "puts"))
    return " ".join(said)


def bullets(lines: list[str]) -> str:
    return "\n".join(textwrap.fill(line, width=WIDTH, initial_indent="- ", subsequent_indent="  ",
                                   break_on_hyphens=False, break_long_words=False) for line in lines)


def entry_of(changelog: str, version: str) -> str:
    """A CHANGELOG entry's text, from its heading to the next release's."""
    start = changelog.find(f"\n## {version} — ")
    if start < 0:
        raise Refused(f"CHANGELOG.md has no entry for {version}")
    following = changelog.find("\n## ", start + 1)
    return changelog[start + 1:following if following >= 0 else len(changelog)]


def tested_paragraph(changelog: str, version: str) -> str:
    for paragraph in re.split(r"\n\s*\n", entry_of(changelog, version)):
        if paragraph.startswith("Tested with"):
            return paragraph.strip("\n")
    raise Refused(f"CHANGELOG.md's {version} entry has no \"Tested with\" paragraph to carry over")


def changelog(text: str, plan: Plan) -> str:
    """CHANGELOG.md with the release's entry above the last release's."""
    if re.search(r"^## Unreleased", text, re.M):
        raise Refused("CHANGELOG.md has an \"## Unreleased\" section; pull requests carry \"Changelog:\" lines instead")
    ships = f"Ships with bridge {plan.bridge}, as {plan.old} did." if plan.same_bridge else RESTART.format(bridge=plan.new_bridge)
    ships = textwrap.fill(" ".join(filter(None, [ships, daws_said(plan)])), width=WIDTH, break_on_hyphens=False)
    parts = [f"## {plan.new} — {plan.date}", ships]
    if plan.kumi:
        parts.append(bullets(plan.kumi))
    parts.append(tested_paragraph(text, plan.old))
    if not plan.same_bridge and plan.bridges:
        parts += [f"### Bridge {plan.new_bridge}", bullets(plan.bridges)]
    for name in plan.moved():
        if plan.daw(name)[2]:
            parts += [f"### {DAWS[name][0]} {plan.daw(name)[1]}", bullets(plan.daw(name)[2])]
    at = text.index(f"\n## {plan.old} — ") + 1
    return text[:at] + "\n\n".join(parts) + "\n\n" + text[at:]


def notes(plan: Plan, tested: str, summary: str | None) -> str:
    if plan.same_bridge:
        ships = f"Ships with bridge **{plan.bridge}**, as {plan.old} did, so Live needs no restart."
        restart = ""
    else:
        ships = f"Ships with bridge **{plan.new_bridge}**, which Live loads when it restarts."
        restart = (" The new bridge needs Live to restart: Kumi offers to quit Live (it asks to save first) and opens"
                   " it again, or run `kumi bridge` with Live closed.")
    ships = " ".join(filter(None, [ships, daws_said(plan, bold=True)]))
    parts = [f"{summary.strip()} {ships}" if summary else f"Kumi {plan.new}. {ships}"]
    if plan.kumi:
        parts.append("\n".join(f"- {line}" for line in plan.kumi))
    if not plan.same_bridge and plan.bridges:
        parts.append(f"**Bridge {plan.new_bridge}**\n" + "\n".join(f"- {line}" for line in plan.bridges))
    for name in plan.moved():
        if plan.daw(name)[2]:
            parts.append(f"**{DAWS[name][0]} {plan.daw(name)[1]}**\n" + "\n".join(f"- {line}" for line in plan.daw(name)[2]))
    parts.append(f"Update with `/update` inside Kumi or `kumi update` in your terminal.{restart}"
                 " `kumi update --rollback` goes back to your previous version.")
    parts.append(" ".join(tested.split()))
    return "\n\n".join(parts) + "\n\n" + MARKER + "\n"


def apply(root: Path, plan: Plan) -> list[str]:
    """The release's edits, in a checkout of main at the last release; the files they change."""
    changed: list[str] = []

    def edit(name: str) -> Path:
        if name not in changed:
            changed.append(name)
        return root / name

    for folder in CRATES.values():
        replace_once(edit(f"{folder}/Cargo.toml"), f'version = "{plan.old}"', f'version = "{plan.new}"')
    replace_once(edit("crates/kumi-types/src/version.rs"), f'"{plan.old}"', f'"{plan.new}"')
    replace_once(edit("package.json"), f'"version": "{plan.old}"', f'"version": "{plan.new}"')
    moves = {crate: (plan.old, plan.new) for crate in CRATES}
    if not plan.same_bridge:
        replace_once(edit("daws/ableton/ableton-mcp-server/Cargo.toml"), f'version = "{plan.bridge}"', f'version = "{plan.new_bridge}"')
        moves["ableton-mcp-server"] = (plan.bridge, plan.new_bridge)
    if not plan.same_reaper:
        replace_once(edit(REAPER_CARGO), f'version = "{plan.reaper}"', f'version = "{plan.new_reaper}"')
        moves[Path(native.REAPER_CRATE).name] = (plan.reaper, plan.new_reaper)
    if not plan.same_renoise:
        try:  # the manifest's number, which Renoise compares: each later version a larger one
            numbers = native.renoise_manifest_version(plan.renoise), native.renoise_manifest_version(plan.new_renoise)
        except ValueError as error:
            raise Refused(str(error)) from None
        replace_once(edit(RENOISE_VERSION), f'return "{plan.renoise}"', f'return "{plan.new_renoise}"')
        replace_once(edit(RENOISE_MANIFEST), f"<Version>{numbers[0]}</Version>", f"<Version>{numbers[1]}</Version>")
    lock = edit("Cargo.lock")
    text = lock.read_text(encoding="utf-8")
    for name, (before, after) in moves.items():
        text, count = re.subn(rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = )"{re.escape(before)}"',
                              rf'\g<1>"{after}"', text)
        if count != 1:
            raise Refused(f"Cargo.lock: expected one {name} {before}, found {count}")
    lock.write_text(text, encoding="utf-8")
    replace_once(edit("README.md"), f"- Kumi {plan.old} is currently supported", f"- Kumi {plan.new} is currently supported")
    replace_once(edit("README.md"), f"- {plan.old} is tested with Live on macOS.", f"- {plan.new} is tested with Live on macOS.")
    replace_once(edit("README.ja.md"), f"Kumi {plan.old} は macOS", f"Kumi {plan.new} は macOS")
    replace_once(edit("README.zh-CN.md"), f"Kumi {plan.old} 已在 macOS", f"Kumi {plan.new} 已在 macOS")
    for language in SHIPS:
        path = edit(f"docs/{language}/KUMI_CHANGES.md")
        path.write_text(ships_with(path.read_text(encoding="utf-8"), language, plan), encoding="utf-8")
    for daw in DAWS:
        if plan.daw(daw)[0] is None:
            continue
        for language, path in daw_lines(root, daw).items():
            name, before = path.relative_to(root).as_posix(), path.read_text(encoding="utf-8")
            text = daw_ships_with(before, daw, language, plan, name)
            if text != before:
                edit(name).write_text(text, encoding="utf-8")
    path = edit("CHANGELOG.md")
    path.write_text(changelog(path.read_text(encoding="utf-8"), plan), encoding="utf-8")
    return changed


def history_docs(root: Path, plan: Plan) -> list[str]:
    """The docs holding REAPER's or Renoise's running ships-with line, which name every version before."""
    return sorted({path.relative_to(root).as_posix() for daw in DAWS if plan.daw(daw)[0] is not None
                   for path in daw_lines(root, daw).values()})


def checks(work: Path, olds: list[str], history: list[str] | None = None) -> list[str]:
    """Refuses unless the lockfile, the whitespace and the packaging tests agree; lists where the old versions are
    left (other crates and packages share numbers, and tests pin versions of their own, so that's for a look). The
    CHANGELOG, KUMI_CHANGES and the `history` docs name every version before, so they're left out."""
    try:
        run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], work)
    except Refused:  # crates not fetched here yet
        run(["cargo", "metadata", "--locked", "--format-version", "1"], work)
    run(["git", "diff", "--check"], work)
    run([sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_native_release.py"], work)
    versions = "|".join(re.escape(old) for old in olds)
    left = subprocess.run(["git", "grep", "-n", "-I", "-E", rf"(^|[^0-9.])({versions})([^0-9]|$)", "--", ".",
                           ":!CHANGELOG.md", ":!docs/*/KUMI_CHANGES.md", *(f":!{name}" for name in history or [])],
                          cwd=work, text=True, capture_output=True)
    return [line[:160] for line in left.stdout.splitlines()]


def bridge_graph(lock_text: str, crate: str = "ableton-mcp-server", own: tuple[str, ...] = ("ableton-mcp-server", "kumi-common")
                 ) -> set[tuple[str, str]]:
    """Every (name, version) a bridge's build reaches in a Cargo.lock (Live's bridge host's, or with `crate` REAPER's
    extension's), less its own crates (whose code BRIDGE or REAPER covers): a change here changes the bridge without a
    file under BRIDGE changing. Test-only dependencies count too, so a difference is for a look rather than a new
    version by itself. Nothing for a lock from before the crate."""
    by_name: dict[str, list[dict]] = {}
    for package in tomllib.loads(lock_text)["package"]:
        by_name.setdefault(package["name"], []).append(package)

    def resolve(dependency: str) -> dict | None:
        name, *version = dependency.split(" ")
        found = [package for package in by_name.get(name, []) if not version or package["version"] == version[0]]
        return found[0] if found else None

    if crate not in by_name:
        return set()
    seen: set[tuple[str, str]] = set()
    stack = [by_name[crate][0]]
    while stack:
        package = stack.pop()
        key = (package["name"], package["version"])
        if key not in seen:
            seen.add(key)
            stack += [found for found in map(resolve, package.get("dependencies", [])) if found]
    return {key for key in seen if key[0] not in own}


def pull_requests(tag: str, head: str, repo: str) -> list[Change]:
    changes = []
    for line in run(["git", "log", "--first-parent", "--reverse", "--format=%H%x09%s", f"{tag}..{head}"]).splitlines():
        sha, subject = line.split("\t", 1)
        found = re.search(r"\(#(\d+)\)$", subject)
        if not found:
            raise Refused(f"{sha[:8]} {subject!r} names no pull request: main takes changes by pull request")
        if re.match(r"Kumi \d+\.\d+\.\d+ \(#\d+\)$", subject):
            raise Refused(f"{subject!r} is a release since {tag} without its tag")
        body = json.loads(run(["gh", "pr", "view", found[1], "--repo", repo, "--json", "body"]))["body"]
        paths = run(["git", "diff", "--name-only", f"{sha}^1", sha]).splitlines()
        changes.append(classify(int(found[1]), body, [now(path) for path in paths]))
    return changes


def pr_body(plan: Plan, changes: list[Change], head: str, left: list[str]) -> str:
    def listed(where: str) -> list[str]:
        return [f"  - #{change.number}: {text}" for change in changes for kind, text in change.lines if kind == where]
    # A bridge without a new version has its lines under Kumi's.
    unmoved = [line for name in DAWS if name not in plan.moved() for line in listed(name)]
    daws = "".join(f" {said}" for said in [daws_said(plan)] if said)
    if plan.same_bridge:
        lines = [f"Kumi {plan.new}. It ships with bridge {plan.bridge}, as {plan.old} did, so Live needs no restart.{daws}",
                 "", "### What ships", "- **Kumi**", *listed("kumi"), *listed("bridge"), *unmoved]
    else:
        kumi = listed("kumi") + unmoved
        lines = [f"Kumi {plan.new}. It ships with bridge {plan.new_bridge}, which Live loads when it restarts.{daws}",
                 "", "### What ships", *(["- **Kumi**", *kumi] if kumi else []),
                 f"- **Bridge {plan.new_bridge}**", *(listed("bridge") or ["  - no line of its own"])]
    for name in plan.moved():
        lines += [f"- **{DAWS[name][0]} {plan.daw(name)[1]}**", *(listed(name) or ["  - no line of its own"])]
    quiet = [f"#{change.number}" for change in changes if not change.lines]
    if quiet:
        lines += ["", f"{', '.join(quiet)}: no changelog line (CI, docs, tests or dependencies)."]
    bridge = "" if plan.same_bridge else f" The bridge goes {plan.bridge} → {plan.new_bridge}."
    moved = [f"- {DAWS[name][0]} {plan.daw(name)[0]} → {plan.daw(name)[1]} in "
             + ("`daws/reaper/reaper-kumi/Cargo.toml` and Cargo.lock." if name == "reaper" else "the tool's `payload/version.lua` and its manifest's Version.")
             for name in plan.moved()]
    lines += [
        "", "### Set here",
        f"- Versions {plan.old} → {plan.new} in the three crates, `version.rs`, `package.json` and Cargo.lock.{bridge}", *moved,
        f"- CHANGELOG `## {plan.new} — {plan.date}`, with each pull request's changelog line as written.",
        "- The README status lines and the KUMI_CHANGES ships-with line, in en, ja and zh-CN.",
        "", f"### Checks (on main {head[:8]} with this commit)",
        "- `cargo metadata --locked`: the lockfile agrees. `git diff --check` is clean. `test_native_release.py`: OK.",
        "- " + " or ".join(f'"{old}"' for old in plan.olds())
        + " left outside CHANGELOG and KUMI_CHANGES (other crates, packages and test fixtures share numbers): "
        + ("none." if not left else ""),
        *[f"  - `{line}`" for line in left[:12]],
        "", "Merged with the admin bypass without waiting for this pull request's CI: the tag build checks the same "
        "commit on six platforms, and main's CI runs on it. Cut by `scripts/release.py`.",
    ]
    return "\n".join(lines) + "\n"


def versions_at(ref: str) -> tuple[str, str | None, str | None]:
    """Live's bridge's version at a commit, REAPER's extension's and Renoise's tool's (None where it has no such one)."""
    def shown(path: str) -> str | None:
        held = next((name for name in then(path) if run(["git", "ls-tree", "--name-only", ref, "--", name])), None)
        return run(["git", "show", f"{ref}:{held}"]) if held else None
    bridge = re.search(r'^version = "([^"]+)"', shown("daws/ableton/ableton-mcp-server/Cargo.toml"), re.M)[1]
    reaper, renoise = shown(REAPER_CARGO), shown(RENOISE_VERSION)
    return (bridge, reaper and re.search(r'^version = "([^"]+)"', reaper, re.M)[1],
            renoise and re.search(r'return "([^"]+)"', renoise)[1])


def plan_release(args: argparse.Namespace, head: str, tag: str, changes: list[Change]) -> Plan:
    old = tag[1:]
    bridge, reaper, renoise = versions_at(head)
    _, reaper_before, renoise_before = versions_at(tag)
    paths = [path for change in changes for path in change.paths]
    changed = args.bridge or (not args.no_bridge and any(map(ships_bridge, paths)))
    new = args.version or bump(old, args.minor)
    if not re.fullmatch(r"\d+\.\d+\.\d+", new) or not newer(new, old):
        raise Refused(f"{new} isn't a version after {old}")
    lines = {kind: [text for change in changes for where, text in change.lines if where == kind] for kind in ("kumi", "bridge", *DAWS)}
    if not changed:  # no new bridge: every line is Kumi's
        lines["kumi"], lines["bridge"] = lines["kumi"] + lines["bridge"], []
    versions, brought = {}, []
    # A DAW's bridge the last release didn't have ships as it is; any other gets a new version when what it loads changed.
    for name, version, before, force, keep, ships in [("reaper", reaper, reaper_before, args.reaper, args.no_reaper, ships_reaper),
                                                      ("renoise", renoise, renoise_before, args.renoise, args.no_renoise, ships_renoise)]:
        if version is None:
            if force:
                raise Refused(f"--{name}: main at {head[:8]} has no {DAWS[name][0]} to give a new version")
            versions[name] = (None, None)
            continue
        if before is None:
            brought.append(name)
        moves = force or (before is not None and not keep and any(map(ships, paths)))
        versions[name] = (version, bump(version) if moves else version)
        if not moves:
            lines["kumi"], lines[name] = lines["kumi"] + lines[name], []
    return Plan(old, new, bridge, bump(bridge) if changed else bridge, args.date or today(), lines["kumi"], lines["bridge"],
                *versions["reaper"], *versions["renoise"], lines["reaper"], lines["renoise"], brought)


def publish(args: argparse.Namespace, repo: str, plan: Plan, changes: list[Change], head: str, work: Path,
            scratch: Path, left: list[str], release_notes: str) -> None:
    branch = f"release/v{plan.new}"
    message = f"Kumi {plan.new}: the changelog, READMEs and versions"
    if args.co_author:
        message += f"\n\nCo-Authored-By: {args.co_author}"
    run(["git", "commit", "--quiet", "-m", message], work)
    commit = run(["git", "rev-parse", "HEAD"], work)
    run(["git", "push", "--quiet", "-u", args.remote, branch], work)
    body = scratch / "pr-body.md"
    body.write_text(pr_body(plan, changes, head, left), encoding="utf-8")
    url = run(["gh", "pr", "create", "--repo", repo, "--base", "main", "--head", branch, "--title", f"Kumi {plan.new}",
               "--body-file", str(body)], work)
    number = url.rstrip("/").rsplit("/", 1)[1]
    print(f"pull request: {url}")
    abandon = f"Close it with `gh pr close {number} --delete-branch`, then run release.py again."
    if args.no_merge:
        print(f"Stopped before merging. Merge #{number} as \"Kumi {plan.new} (#{number})\", then tag it v{plan.new}.")
        return
    # What came into main meanwhile isn't in the CHANGELOG: the release is cut again from the new main instead.
    run(["git", "fetch", "--quiet", args.remote, "main"])
    if run(["git", "rev-parse", f"{args.remote}/main"]) != head:
        raise Refused(f"main moved since {head[:8]}, so #{number} isn't merged. {abandon}")
    try:
        run(["gh", "pr", "merge", number, "--repo", repo, "--merge", "--admin", "--match-head-commit", commit,
             "--subject", f"Kumi {plan.new} (#{number})", "--body", ""])
    except Refused as refused:
        raise Refused(f"{refused}\n#{number} isn't merged. {abandon}")
    merged = json.loads(run(["gh", "pr", "view", number, "--repo", repo, "--json", "mergeCommit"]))["mergeCommit"]["oid"]
    run(["git", "fetch", "--quiet", args.remote, "main"])
    notes_file = Path(tempfile.gettempdir()) / f"kumi-{plan.new}-release-notes.md"
    notes_file.write_text(release_notes, encoding="utf-8")
    finish = (f"`git tag v{plan.new} {merged[:8]}`, `git push {args.remote} v{plan.new}` and `gh release create "
              f"v{plan.new} --draft --title \"Kumi {plan.new}\" --notes-file {notes_file} --verify-tag`")
    # A pull request can still land between the check above and the merge (auto-merge is on): it would ship in the
    # tag without its changelog line, and a bridge change in it without a new bridge version.
    parent = run(["git", "rev-parse", f"{merged}^1"])
    if parent != head:
        came = run(["git", "log", "--first-parent", "--format=%s", f"{head}..{parent}"]).splitlines()
        raise Refused(f"main moved while #{number} merged, so {merged[:8]} isn't tagged: {'; '.join(came)} came in, "
                      f"and would ship in v{plan.new} without their changelog lines (or a bridge version, if they "
                      f"change the bridge). To finish, add their lines to {notes_file}, then {finish}")
    try:
        run(["git", "tag", f"v{plan.new}", merged])
        run(["git", "push", "--quiet", args.remote, f"v{plan.new}"])
    except Refused as refused:
        raise Refused(f"{refused}\n#{number} is merged as {merged[:8]}. Finish with {finish}")
    try:
        run(["gh", "release", "create", f"v{plan.new}", "--repo", repo, "--draft", "--title", f"Kumi {plan.new}",
             "--notes-file", str(notes_file), "--verify-tag"])
    except Refused:  # the tag build made the draft first, with notes to come
        try:
            run(["gh", "release", "edit", f"v{plan.new}", "--repo", repo, "--notes-file", str(notes_file)])
        except Refused as refused:
            raise Refused(f"{refused}\nv{plan.new} is pushed but its draft has no notes, so the tag build won't "
                          f"publish it. Finish with `gh release edit v{plan.new} --notes-file {notes_file}`")
    print(f"Kumi {plan.new}: #{number} merged as {merged[:8]}, tagged v{plan.new}, its draft written. The tag build "
          f"publishes it once every install passes: https://github.com/{repo}/actions/workflows/installer.yml")


def release(args: argparse.Namespace) -> None:
    repo = run(["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
    run(["git", "fetch", "--quiet", "--tags", args.remote, "main"])
    if args.at:  # main as it was at a commit: the last tag before it is the last release
        head = run(["git", "rev-parse", f"{args.at}^{{commit}}"])
        tag = run(["git", "describe", "--tags", "--abbrev=0", "--match", "v*", head])
    else:
        head = run(["git", "rev-parse", f"{args.remote}/main"])
        tag = run(["git", "tag", "--list", "v*", "--sort=-v:refname"]).split()[0]
    on_main = json.loads(run(["git", "show", f"{head}:package.json"]))["version"]
    if f"v{on_main}" != tag:
        raise Refused(f"main says Kumi {on_main}, but the newest tag is {tag}: tag v{on_main} first")
    changes = pull_requests(tag, head, repo)
    if not changes:
        raise Refused(f"nothing has merged since {tag}")
    plan = plan_release(args, head, tag, changes)
    for change in changes:
        if not change.written:
            print(f"note: #{change.number} has no Changelog: line, so nothing of it is listed", file=sys.stderr)
        for text in change.wrapped:
            print(f"LOOK: #{change.number}'s Changelog line seems to go on past its line break, and only its first row "
                  f"is read: {text[:70]!r}. Put it on one row in the pull request's body.", file=sys.stderr)
        headings = {"bridge": None if plan.same_bridge else f"Bridge {plan.new_bridge}",
                    **{name: f"{DAWS[name][0]} {plan.daw(name)[1]}" for name in plan.moved()}}
        for kind, text in change.lines:
            print(f"#{change.number} → {headings.get(kind) or 'Kumi'}: {text[:100]}")
    if not (plan.kumi or plan.bridges or plan.reaper_lines or plan.renoise_lines):
        raise Refused(f"no pull request since {tag} has a changelog line, so there's nothing to release")
    locks = {ref: run(["git", "show", f"{ref}:Cargo.lock"]) for ref in (tag, head)}
    reaper = Path(native.REAPER_CRATE).name
    for crate, own, whose, same, flag in [("ableton-mcp-server", ("ableton-mcp-server", "kumi-common"), "the bridge host's", plan.same_bridge, "--bridge"),
                                          (reaper, (reaper, "daw-bridge"), "REAPER's extension's", plan.same_reaper, "--reaper")]:
        before, after = (bridge_graph(locks[ref], crate, own) for ref in (tag, head))
        if before and before != after:  # (nothing to compare with in a lock from before the crate)
            moved = [f"+ {name} {version}" for name, version in sorted(after - before)]
            moved += [f"- {name} {version}" for name, version in sorted(before - after)]
            advice = "it gets a new version anyway" if not same else f"if its build uses them, cut with {flag}"
            print(f"LOOK: {whose} dependencies changed since {tag} ({advice}):\n  " + "\n  ".join(moved), file=sys.stderr)
    if args.go:  # admin before anything is pushed: the merge goes through the admin bypass, and tags are admin-only
        if run(["gh", "api", f"repos/{repo}", "--jq", ".permissions.admin"]) != "true":
            raise Refused(f"gh isn't signed in as an admin of {repo}, which the merge and the tag need")

    scratch = Path(tempfile.mkdtemp(prefix=f"kumi-release-{plan.new}-"))
    work = scratch / "worktree"
    branch = f"release/v{plan.new}"
    run(["git", "worktree", "add", "--quiet", *(["-b", branch] if args.go else ["--detach"]), str(work), head])
    try:
        for name in DAWS:
            if plan.daw(name)[0] is not None and not daw_lines(work, name):
                example = DAW_SHIPS[name]["en"][1].format(range=plan.new, bridge=plan.daw(name)[1], end=".")
                print(f"LOOK: no doc has the {DAWS[name][0]}'s ships-with line yet (such as {example!r}, in en, ja and "
                      f"zh-CN), so none is moved on", file=sys.stderr)
        changed = apply(work, plan)
        left = checks(work, plan.olds(), history_docs(work, plan))
        changelog_text = (work / "CHANGELOG.md").read_text(encoding="utf-8")
        release_notes = notes(plan, tested_paragraph(changelog_text, plan.new), args.summary)
        bridge = f"{plan.bridge} (unchanged)" if plan.same_bridge else f"{plan.bridge} → {plan.new_bridge}"
        daws = "".join(f", {DAWS[name][0]} " + (f"{old} (first release)" if name in plan.brought
                                                else f"{old} (unchanged)" if old == new else f"{old} → {new}")
                       for name in DAWS for old, new, _ in [plan.daw(name)] if old is not None)
        print(f"Kumi {plan.old} → {plan.new}, bridge {bridge}{daws}, dated {plan.date}, from main {head[:8]} with "
              + " ".join(f"#{change.number}" for change in changes))
        print("\n" + entry_of(changelog_text, plan.new).rstrip())
        print("\n--- release notes\n" + release_notes)
        olds = " or ".join(plan.olds())
        print(f"--- {olds} left outside CHANGELOG and KUMI_CHANGES (for a look): " + ("none" if not left else "\n" + "\n".join(left)))
        run(["git", "add", "-A"], work)
        files = run(["git", "diff", "--cached", "--name-only"], work).splitlines()
        if sorted(files) != sorted(changed):
            raise Refused(f"the release's edits and the files git sees changed differ: {', '.join(sorted(set(files) ^ set(changed)))}")
        if args.go:
            publish(args, repo, plan, changes, head, work, scratch, left, release_notes)
        else:
            print(f"\nDry run: nothing committed or pushed. `python3 scripts/release.py --go` releases Kumi {plan.new}.")
    finally:
        if args.keep:
            print(f"\nKept the worktree with the edits: {work} (`git worktree remove --force {work}` when done)")
        else:
            subprocess.run(["git", "worktree", "remove", "--force", str(work)], cwd=ROOT, capture_output=True)
            if args.go:  # pushed if it got that far; the local branch isn't needed either way
                subprocess.run(["git", "branch", "-D", branch], cwd=ROOT, capture_output=True)
            shutil.rmtree(scratch, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--go", action="store_true", help="release for real (without it, a dry run)")
    which = parser.add_mutually_exclusive_group()
    which.add_argument("--minor", action="store_true", help="X.Y+1.0 instead of the next patch")
    which.add_argument("--version", help="this version instead of the next patch")
    bridge = parser.add_mutually_exclusive_group()
    bridge.add_argument("--bridge", action="store_true", help="give the bridge a new version whatever changed")
    bridge.add_argument("--no-bridge", action="store_true", help="keep the bridge's version whatever changed")
    reaper = parser.add_mutually_exclusive_group()
    reaper.add_argument("--reaper", action="store_true", help="give REAPER's extension a new version whatever changed")
    reaper.add_argument("--no-reaper", action="store_true", help="keep REAPER's extension's version whatever changed")
    renoise = parser.add_mutually_exclusive_group()
    renoise.add_argument("--renoise", action="store_true", help="give Renoise's tool a new version whatever changed")
    renoise.add_argument("--no-renoise", action="store_true", help="keep Renoise's tool's version whatever changed")
    parser.add_argument("--date", help="the CHANGELOG's date (today in New York by default)")
    parser.add_argument("--summary", help="a sentence to open the release notes with")
    parser.add_argument("--co-author", help='"Name <email>" for the release commit\'s Co-Authored-By trailer')
    parser.add_argument("--no-merge", action="store_true", help="with --go, stop once the pull request is open")
    parser.add_argument("--remote", default="origin")
    parser.add_argument("--at", metavar="COMMIT", help="dry run only: as if main were at COMMIT")
    parser.add_argument("--keep", action="store_true", help="dry run only: keep the worktree with the edits")
    args = parser.parse_args()
    if args.go and (args.at or args.keep):
        parser.error("--at and --keep are for dry runs")
    try:
        release(args)
    except Refused as refused:
        raise SystemExit(f"release.py: {refused}")


if __name__ == "__main__":
    main()
