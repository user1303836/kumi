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
The version goes up by a patch, by a minor with --minor, or is given with --version.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import datetime
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
# A change under these, besides tests and Markdown, is a new bridge for Live to load. The host links kumi-common.
BRIDGE = ("crates/ableton-mcp-server/", "crates/kumi-common/", "remote-script/", "protocol/", "apps/live-extension/",
          "vendor/willington/")
KUMI = ("crates/kumi/", "crates/kumi-runtime/", "crates/kumi-common/", "crates/kumi-store/", "install.sh", "install.ps1")
CRATES = ("kumi", "kumi-common", "kumi-runtime")
# The CHANGELOG's dates are New York's, as its earlier entries are.
TIMEZONE = "America/New_York"
WIDTH = 102
LINE = re.compile(r"^Changelog(?:\s*\((?P<where>kumi|bridge)\))?:\s*(?P<text>.+?)\s*$", re.M | re.I)
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


class Refused(Exception):
    """A release that can't go ahead as things are; the message says why."""


@dataclass
class Change:
    number: int
    paths: list[str]
    lines: list[tuple[str, str]]  # (kumi | bridge, text)
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

    @property
    def same_bridge(self) -> bool:
        return self.new_bridge == self.bridge


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


def ships_bridge(path: str) -> bool:
    name = path.rsplit("/", 1)[-1]
    return path.startswith(BRIDGE) and not path.endswith(".md") and "/tests/" not in path and not name.startswith("test_")


def classify(number: int, body: str, paths: list[str]) -> Change:
    rows = (body or "").replace("\r\n", "\n").split("\n")
    found = [(i, m) for i, row in enumerate(rows) if (m := LINE.match(row))]
    bridge_only = any(ships_bridge(path) for path in paths) and not any(path.startswith(KUMI) for path in paths)
    lines = [((m["where"] or ("bridge" if bridge_only else "kumi")).lower(), m["text"])
             for _, m in found if not re.match(r"none\b", m["text"], re.I)]
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
    if plan.same_bridge:
        ships = f"Ships with bridge {plan.bridge}, as {plan.old} did."
    else:
        ships = textwrap.fill(RESTART.format(bridge=plan.new_bridge), width=WIDTH, break_on_hyphens=False)
    parts = [f"## {plan.new} — {plan.date}", ships]
    if plan.kumi:
        parts.append(bullets(plan.kumi))
    parts.append(tested_paragraph(text, plan.old))
    if not plan.same_bridge and plan.bridges:
        parts += [f"### Bridge {plan.new_bridge}", bullets(plan.bridges)]
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
    parts = [f"{summary.strip()} {ships}" if summary else f"Kumi {plan.new}. {ships}"]
    if plan.kumi:
        parts.append("\n".join(f"- {line}" for line in plan.kumi))
    if not plan.same_bridge and plan.bridges:
        parts.append(f"**Bridge {plan.new_bridge}**\n" + "\n".join(f"- {line}" for line in plan.bridges))
    parts.append(f"Update with `/update` inside Kumi or `kumi update` in your terminal.{restart}"
                 " `kumi update --rollback` goes back to your previous version.")
    parts.append(" ".join(tested.split()))
    return "\n\n".join(parts) + "\n\n" + MARKER + "\n"


def apply(root: Path, plan: Plan) -> None:
    """The release's edits, in a checkout of main at the last release."""
    for crate in CRATES:
        replace_once(root / f"crates/{crate}/Cargo.toml", f'version = "{plan.old}"', f'version = "{plan.new}"')
    replace_once(root / "crates/kumi-runtime/src/version.rs", f'"{plan.old}"', f'"{plan.new}"')
    replace_once(root / "package.json", f'"version": "{plan.old}"', f'"version": "{plan.new}"')
    moves = {crate: (plan.old, plan.new) for crate in CRATES}
    if not plan.same_bridge:
        replace_once(root / "crates/ableton-mcp-server/Cargo.toml", f'version = "{plan.bridge}"', f'version = "{plan.new_bridge}"')
        moves["ableton-mcp-server"] = (plan.bridge, plan.new_bridge)
    lock = root / "Cargo.lock"
    text = lock.read_text(encoding="utf-8")
    for name, (before, after) in moves.items():
        text, count = re.subn(rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = )"{re.escape(before)}"',
                              rf'\g<1>"{after}"', text)
        if count != 1:
            raise Refused(f"Cargo.lock: expected one {name} {before}, found {count}")
    lock.write_text(text, encoding="utf-8")
    replace_once(root / "README.md", f"- Kumi {plan.old} is currently supported", f"- Kumi {plan.new} is currently supported")
    replace_once(root / "README.md", f"- {plan.old} is tested with Live on macOS.", f"- {plan.new} is tested with Live on macOS.")
    replace_once(root / "README.ja.md", f"Kumi {plan.old} は macOS", f"Kumi {plan.new} は macOS")
    replace_once(root / "README.zh-CN.md", f"Kumi {plan.old} 已在 macOS", f"Kumi {plan.new} 已在 macOS")
    for language in SHIPS:
        path = root / f"docs/{language}/KUMI_CHANGES.md"
        path.write_text(ships_with(path.read_text(encoding="utf-8"), language, plan), encoding="utf-8")
    path = root / "CHANGELOG.md"
    path.write_text(changelog(path.read_text(encoding="utf-8"), plan), encoding="utf-8")


def checks(work: Path, olds: list[str]) -> list[str]:
    """Refuses unless the lockfile, the whitespace and the packaging tests agree; lists where the old versions are
    left (other crates and packages share numbers, and tests pin versions of their own, so that's for a look)."""
    try:
        run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], work)
    except Refused:  # crates not fetched here yet
        run(["cargo", "metadata", "--locked", "--format-version", "1"], work)
    run(["git", "diff", "--check"], work)
    run([sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_native_release.py"], work)
    versions = "|".join(re.escape(old) for old in olds)
    left = subprocess.run(["git", "grep", "-n", "-I", "-E", rf"(^|[^0-9.])({versions})([^0-9]|$)", "--", ".",
                           ":!CHANGELOG.md", ":!docs/*/KUMI_CHANGES.md"], cwd=work, text=True, capture_output=True)
    return [line[:160] for line in left.stdout.splitlines()]


def bridge_graph(lock_text: str) -> set[tuple[str, str]]:
    """Every (name, version) the bridge host's build reaches in a Cargo.lock, less its own crates (whose code BRIDGE
    covers): a change here changes the bridge without a file under BRIDGE changing. Test-only dependencies count
    too, so a difference is for a look rather than a new version by itself."""
    by_name: dict[str, list[dict]] = {}
    for package in tomllib.loads(lock_text)["package"]:
        by_name.setdefault(package["name"], []).append(package)

    def resolve(dependency: str) -> dict | None:
        name, *version = dependency.split(" ")
        found = [package for package in by_name.get(name, []) if not version or package["version"] == version[0]]
        return found[0] if found else None

    seen: set[tuple[str, str]] = set()
    stack = [by_name["ableton-mcp-server"][0]]
    while stack:
        package = stack.pop()
        key = (package["name"], package["version"])
        if key not in seen:
            seen.add(key)
            stack += [found for found in map(resolve, package.get("dependencies", [])) if found]
    return {key for key in seen if key[0] not in ("ableton-mcp-server", "kumi-common")}


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
        changes.append(classify(int(found[1]), body, run(["git", "diff", "--name-only", f"{sha}^1", sha]).splitlines()))
    return changes


def pr_body(plan: Plan, changes: list[Change], head: str, left: list[str]) -> str:
    def listed(where: str) -> list[str]:
        return [f"  - #{change.number}: {text}" for change in changes for kind, text in change.lines if kind == where]
    if plan.same_bridge:
        lines = [f"Kumi {plan.new}. It ships with bridge {plan.bridge}, as {plan.old} did, so Live needs no restart.",
                 "", "### What ships", "- **Kumi**", *listed("kumi"), *listed("bridge")]
    else:
        lines = [f"Kumi {plan.new}. It ships with bridge {plan.new_bridge}, which Live loads when it restarts.",
                 "", "### What ships", *(["- **Kumi**", *listed("kumi")] if listed("kumi") else []),
                 f"- **Bridge {plan.new_bridge}**", *(listed("bridge") or ["  - no line of its own"])]
    quiet = [f"#{change.number}" for change in changes if not change.lines]
    if quiet:
        lines += ["", f"{', '.join(quiet)}: no changelog line (CI, docs, tests or dependencies)."]
    bridge = "" if plan.same_bridge else f" The bridge goes {plan.bridge} → {plan.new_bridge}."
    lines += [
        "", "### Set here",
        f"- Versions {plan.old} → {plan.new} in the three crates, `version.rs`, `package.json` and Cargo.lock.{bridge}",
        f"- CHANGELOG `## {plan.new} — {plan.date}`, with each pull request's changelog line as written.",
        "- The README status lines and the KUMI_CHANGES ships-with line, in en, ja and zh-CN.",
        "", f"### Checks (on main {head[:8]} with this commit)",
        "- `cargo metadata --locked`: the lockfile agrees. `git diff --check` is clean. `test_native_release.py`: OK.",
        f"- \"{plan.old}\"" + ("" if plan.same_bridge else f" or \"{plan.bridge}\"")
        + " left outside CHANGELOG and KUMI_CHANGES (other crates, packages and test fixtures share numbers): "
        + ("none." if not left else ""),
        *[f"  - `{line}`" for line in left[:12]],
        "", "Merged with the admin bypass without waiting for this pull request's CI: the tag build checks the same "
        "commit on six platforms, and main's CI runs on it. Cut by `scripts/release.py`.",
    ]
    return "\n".join(lines) + "\n"


def plan_release(args: argparse.Namespace, head: str, tag: str, changes: list[Change]) -> Plan:
    old = tag[1:]
    bridge = re.search(r'^version = "([^"]+)"', run(["git", "show", f"{head}:crates/ableton-mcp-server/Cargo.toml"]), re.M)[1]
    changed = args.bridge or (not args.no_bridge and any(ships_bridge(path) for change in changes for path in change.paths))
    new = args.version or bump(old, args.minor)
    if not re.fullmatch(r"\d+\.\d+\.\d+", new) or not newer(new, old):
        raise Refused(f"{new} isn't a version after {old}")
    kumi = [text for change in changes for kind, text in change.lines if kind == "kumi"]
    bridges = [text for change in changes for kind, text in change.lines if kind == "bridge"]
    if not changed:  # no new bridge: every line is Kumi's
        kumi, bridges = kumi + bridges, []
    return Plan(old, new, bridge, bump(bridge) if changed else bridge, args.date or today(), kumi, bridges)


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
        for kind, text in change.lines:
            heading = f"Bridge {plan.new_bridge}" if kind == "bridge" and not plan.same_bridge else "Kumi"
            print(f"#{change.number} → {heading}: {text[:100]}")
    if not plan.kumi and not plan.bridges:
        raise Refused(f"no pull request since {tag} has a changelog line, so there's nothing to release")
    before, after = (bridge_graph(run(["git", "show", f"{ref}:Cargo.lock"])) for ref in (tag, head))
    if before != after:
        moved = [f"+ {name} {version}" for name, version in sorted(after - before)]
        moved += [f"- {name} {version}" for name, version in sorted(before - after)]
        advice = "it gets a new version anyway" if not plan.same_bridge else "if the host's build uses them, cut with --bridge"
        print(f"LOOK: the bridge host's dependencies changed since {tag} ({advice}):\n  " + "\n  ".join(moved), file=sys.stderr)
    if args.go:  # admin before anything is pushed: the merge goes through the admin bypass, and tags are admin-only
        if run(["gh", "api", f"repos/{repo}", "--jq", ".permissions.admin"]) != "true":
            raise Refused(f"gh isn't signed in as an admin of {repo}, which the merge and the tag need")

    scratch = Path(tempfile.mkdtemp(prefix=f"kumi-release-{plan.new}-"))
    work = scratch / "worktree"
    branch = f"release/v{plan.new}"
    run(["git", "worktree", "add", "--quiet", *(["-b", branch] if args.go else ["--detach"]), str(work), head])
    try:
        apply(work, plan)
        left = checks(work, [plan.old] + ([] if plan.same_bridge else [plan.bridge]))
        changelog_text = (work / "CHANGELOG.md").read_text(encoding="utf-8")
        release_notes = notes(plan, tested_paragraph(changelog_text, plan.new), args.summary)
        bridge = f"{plan.bridge} (unchanged)" if plan.same_bridge else f"{plan.bridge} → {plan.new_bridge}"
        print(f"Kumi {plan.old} → {plan.new}, bridge {bridge}, dated {plan.date}, from main {head[:8]} with "
              + " ".join(f"#{change.number}" for change in changes))
        print("\n" + entry_of(changelog_text, plan.new).rstrip())
        print("\n--- release notes\n" + release_notes)
        olds = plan.old if plan.same_bridge else f"{plan.old} or {plan.bridge}"
        print(f"--- {olds} left outside CHANGELOG and KUMI_CHANGES (for a look): " + ("none" if not left else "\n" + "\n".join(left)))
        run(["git", "add", "-A"], work)
        files = run(["git", "diff", "--cached", "--name-only"], work).splitlines()
        expected = 13 if plan.same_bridge else 14
        if len(files) != expected:
            raise Refused(f"the release changes {len(files)} files, not {expected}: {', '.join(files)}")
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
