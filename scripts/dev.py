#!/usr/bin/env python3
"""Kumi's development tasks (Python 3.11+, standard library only; git, and cargo for the packages).

    python3 scripts/dev.py affected --base origin/main    # what a change affects, and the CI jobs that test it
    python3 scripts/dev.py affected --base origin/main --json
    python3 scripts/dev.py affected --base origin/main --job installer    # true or false
    python3 scripts/dev.py affected                       # no base: everything (a push to main)
    python3 scripts/dev.py affected --check               # every tracked file belongs to a package or an area
    python3 scripts/dev.py areas [--in AREA] PATH...      # the areas each path belongs to
    python3 scripts/dev.py check [--base origin/main]     # Required CI's checks for what this branch changes
    python3 scripts/dev.py select "PACKAGES" [-p NAME | --exclude NAME]...   # a CI part's affected packages, as -p options
    python3 scripts/dev.py rebase-moves [--onto origin/main] [--merge] [--no-build]   # a branch from before a move, onto main

`affected` reads the changed files from `git diff <base>...HEAD`. A file's package is the Cargo package whose folder
holds it (the longest such folder); its areas are the ones in scripts/areas.toml whose globs match it. The affected
packages are those packages and the ones an area feeds, then every package that depends on them (a dev-dependency's
dependent once, for its tests). A change that would run Rust's jobs but that no package holds and no area feeds (a
workflow, a script, git's settings) affects every package: nothing says which it doesn't. The jobs are CI's, by their
ids in .github/workflows:

- rust_linux, rust_lint, rust_platforms: any change but to docs, or to the Renoise tool beyond what Kumi reads of it;
- remote_script, live_extension: any change but to docs;
- reaper_extension: REAPER's extension, a package it builds with, or CI's workflow;
- renoise_tool: the Renoise tool, or CI's workflow;
- installer: the install surface (a pull request's Installer runs only for it);
- network: how Kumi's HTTP clients are made.

`check` runs, in CI's order and for what the branch changes since `--base` (origin/main): the change's whitespace and
the areas' check; then, when it affects Rust packages, `cargo fmt --all --check`, the gating Clippy lint and the `ci`
tests (cargo-nextest, through scripts/test-isolated) of those packages, the ones CI's Rust jobs test; then the Remote
Script's, the Live extension's and the Renoise tool's tests when their areas changed (each skipped, and said, without its
npm or LuaJIT); and the release scripts' and the installers' checks (the latter skipped without shellcheck and pwsh).

`select` takes the affected packages (space-separated, as `affected --github-output` writes them) and a part of the
workspace as cargo selects it (`-p` each package, or `--exclude` the rest's), and prints `-p` for each of that part's
affected packages: the ones a CI job of that part tests. It prints nothing when none is.

`rebase-moves` brings a branch that began before a move in scripts/moves.toml onto `--onto` (origin/main). It rebases
the branch (or merges `--onto` into it, `--merge`) with git following moved folders, files the branch adds in them
included (merge.directoryRenames); rewrites each moved repository path (`[[move]]`) and Rust path (`[[rust]]`) in the
lines the branch adds, leaving them for you to review and commit; lists the files the branch adds where a path moved
away (git follows a moved folder, not files moved out of one); and builds every target (`cargo check`, skipped with
`--no-build`), listing the errors. It reads the map from `--onto` and works on the checkout it's run in, so a branch
older than this command runs main's copy: `python3 <(git show origin/main:scripts/dev.py) rebase-moves`. When the rebase
stops on a conflict, resolve it, `git rebase --continue`, and run it again.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
AREAS = ROOT / "scripts/areas.toml"
# REAPER's extension, which CI builds as it ships and loads on each system.
REAPER_EXTENSION = "reaper-kumi"
JOBS = ("rust_linux", "rust_lint", "rust_platforms", "remote_script", "live_extension", "reaper_extension",
        "renoise_tool", "installer", "network")
# Where the repository's paths moved, oldest first: `rebase-moves` reads it from the branch it rebases onto.
MOVES = "scripts/moves.toml"
WORD = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_")
PATH = WORD | frozenset("-./")


class Refused(Exception):
    """A request dev.py can't do, said in a line."""


def glob(pattern: str) -> re.Pattern[str]:
    """A glob over a whole path from the repository's root: `*` and `?` stay inside a folder, `**` crosses folders."""
    out, at = [], 0
    while at < len(pattern):
        if pattern.startswith("**/", at):
            out.append("(?:.*/)?")
            at += 3
        elif pattern.startswith("**", at):
            out.append(".*")
            at += 2
        elif pattern[at] == "*":
            out.append("[^/]*")
            at += 1
        elif pattern[at] == "?":
            out.append("[^/]")
            at += 1
        else:
            out.append(re.escape(pattern[at]))
            at += 1
    return re.compile("".join(out) + r"\Z")


@dataclass
class Area:
    name: str
    paths: list[re.Pattern[str]]
    feeds: list[str] = field(default_factory=list)
    unless: list[re.Pattern[str]] = field(default_factory=list)
    layout: list[re.Pattern[str]] = field(default_factory=list)

    def holds(self, path: str, moved: bool = False) -> bool:
        """Whether `path` is one of this area's: a file it names, or one added, removed or renamed in its layout."""
        if any(pattern.match(path) for pattern in self.unless):
            return False
        return any(pattern.match(path) for pattern in self.paths) or (moved and any(p.match(path) for p in self.layout))


def load_areas(path: Path = AREAS) -> list[Area]:
    areas = []
    for name, table in tomllib.loads(path.read_text(encoding="utf-8")).items():
        unknown = set(table) - {"paths", "feeds", "except", "layout"}
        if unknown or not table.get("paths"):
            raise Refused(f"{path.name}: [{name}] needs paths, and takes only feeds, except and layout besides")
        areas.append(Area(name, [glob(p) for p in table["paths"]], list(table.get("feeds", [])),
                          [glob(p) for p in table.get("except", [])], [glob(p) for p in table.get("layout", [])]))
    return areas


@dataclass
class Workspace:
    """The Cargo packages: each one's folder from the root, and the packages it depends on (and how)."""
    folders: dict[str, str]
    depends: dict[str, list[tuple[str, str]]]

    @staticmethod
    def from_metadata(metadata: dict) -> Workspace:
        root = Path(metadata["workspace_root"])

        def folder(path: str) -> str:
            return Path(os.path.relpath(path, root)).as_posix()

        folders = {package["name"]: folder(Path(package["manifest_path"]).parent) for package in metadata["packages"]}
        named = {place: name for name, place in folders.items()}
        depends = {name: [] for name in folders}
        for package in metadata["packages"]:
            for dependency in package["dependencies"]:
                member = named.get(folder(dependency["path"])) if dependency.get("path") else None
                if member:
                    depends[package["name"]].append((member, dependency.get("kind") or "normal"))
        return Workspace(folders, depends)

    @staticmethod
    def read(root: Path = ROOT) -> Workspace:
        try:
            out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"], cwd=root,
                                 capture_output=True, text=True, check=True).stdout
        except FileNotFoundError:
            raise Refused("cargo isn't on PATH: dev.py reads the packages from `cargo metadata`") from None
        except subprocess.CalledProcessError as failed:
            raise Refused(f"cargo metadata failed: {failed.stderr.strip()}") from None
        return Workspace.from_metadata(json.loads(out))

    def package_of(self, path: str) -> str | None:
        """The package whose folder holds `path`: the deepest, when one holds another."""
        holders = [name for name, place in self.folders.items() if path.startswith(place + "/")]
        return max(holders, key=lambda name: len(self.folders[name]), default=None)

    def dependents(self, changed: set[str]) -> set[str]:
        """`changed`, every package that depends on them, and each dev-dependent of those: its tests see the change,
        and nothing builds on its tests."""
        built = set(changed)
        while True:
            more = {name for name, uses in self.depends.items()
                    if name not in built and any(kind != "dev" and used in built for used, kind in uses)}
            if not more:
                break
            built |= more
        return built | {name for name, uses in self.depends.items() if any(kind == "dev" and used in built for used, kind in uses)}


@dataclass
class Affected:
    files: int
    packages: list[str]
    areas: list[str]
    jobs: dict[str, bool]

    def as_json(self) -> dict:
        return {"files": self.files, "packages": self.packages, "areas": self.areas, "jobs": self.jobs}


def changes_since(base: str, root: Path = ROOT) -> list[tuple[str, list[str]]]:
    """Each changed file since `base`'s merge base with HEAD: its status letter (A, D, M, R…) and its path, or for a
    rename its old and new paths."""
    try:
        out = subprocess.run(["git", "diff", "--name-status", "-M", f"{base}...HEAD"], cwd=root, capture_output=True,
                             text=True, check=True).stdout
    except subprocess.CalledProcessError as failed:
        raise Refused(f"git diff {base}...HEAD failed: {failed.stderr.strip()}") from None
    return parse_name_status(out)


def parse_name_status(text: str) -> list[tuple[str, list[str]]]:
    changes = []
    for line in text.splitlines():
        if line.strip():
            status, *paths = line.split("\t")
            changes.append((status[0], paths))
    return changes


def affected(changes: list[tuple[str, list[str]]] | None, workspace: Workspace, areas: list[Area]) -> Affected:
    """What `changes` affect, and the jobs that test it. None: everything (a push to main, or a run by hand)."""
    if changes is None:
        return Affected(0, sorted(workspace.folders), sorted(area.name for area in areas), {job: True for job in JOBS})
    owners, fed, hit = set(), set(), set()
    code = rust = renoise = reaper = installer = network = unfed = False
    files = 0
    for status, paths in changes:
        for path in paths:
            files += 1
            names = {area.name for area in areas if area.holds(path, moved=status in "ADR")}
            hit |= names
            # Words alone build and test nothing, even inside a package's folder.
            docs = "docs" in names
            owner = None if docs else workspace.package_of(path)
            feeds = set()
            if not docs:
                for area in areas:
                    if area.name in names:
                        feeds |= set(workspace.folders) if "*" in area.feeds else set(area.feeds)
            owners |= {owner} if owner else set()
            fed |= feeds
            tests_rust = not docs and ("renoise-tool" not in names or "renoise-tool-read" in names)
            code |= not docs
            rust |= tests_rust
            unfed |= tests_rust and not owner and not feeds
            renoise |= bool(names & {"renoise-tool", "ci"})
            reaper |= bool(names & {"reaper-extension", "ci"})
            installer |= "install" in names
            network |= "network" in names
    unknown = fed - set(workspace.folders)
    if unknown:
        raise Refused(f"areas.toml feeds packages the workspace doesn't have: {', '.join(sorted(unknown))}")
    named = workspace.dependents(owners | fed)
    packages = set(workspace.folders) if unfed else named
    # The extension's own jobs follow what it builds with, never the everything above.
    reaper |= REAPER_EXTENSION in named
    jobs = {"rust_linux": rust, "rust_lint": rust, "rust_platforms": rust, "remote_script": code, "live_extension": code,
            "reaper_extension": reaper, "renoise_tool": renoise, "installer": installer, "network": network}
    return Affected(files, sorted(packages), sorted(hit), jobs)


def tracked(root: Path = ROOT) -> list[str]:
    return subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True, text=True, check=True).stdout.split("\0")[:-1]


def unplaced(paths: list[str], workspace: Workspace, areas: list[Area]) -> list[str]:
    """The paths that belong to no package and no area."""
    return [path for path in paths if not workspace.package_of(path) and not any(area.holds(path) for area in areas)]


def command_affected(args: argparse.Namespace) -> int:
    areas, workspace = load_areas(), Workspace.read()
    if args.check:
        left = unplaced(tracked(), workspace, areas)
        if left:
            print(f"{len(left)} tracked files belong to no package and no area in scripts/areas.toml:", file=sys.stderr)
            for path in left:
                print(f"  {path}", file=sys.stderr)
            return 1
        print(f"Every tracked file belongs to a package or an area ({len(workspace.folders)} packages, {len(areas)} areas).")
        return 0
    if args.changes is not None:
        changes = parse_name_status(sys.stdin.read() if args.changes == "-" else Path(args.changes).read_text())
    elif args.base and args.base.strip("0"):
        changes = changes_since(args.base)
    else:
        changes = None
    result = affected(changes, workspace, areas)
    if args.github_output:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as out:
            for job, runs in result.jobs.items():
                out.write(f"{job}={'true' if runs else 'false'}\n")
            out.write(f"packages={' '.join(result.packages)}\nareas={' '.join(result.areas)}\n")
    if args.job:
        print("true" if result.jobs[args.job] else "false")
    elif args.json:
        print(json.dumps(result.as_json(), indent=2))
    else:
        print("Everything:" if changes is None else f"{result.files} changed file{'' if result.files == 1 else 's'}:")
        print(f"  packages: {', '.join(result.packages) or 'none'}")
        print(f"  areas: {', '.join(result.areas) or 'none'}")
        print(f"  jobs: {', '.join(job for job, runs in result.jobs.items() if runs) or 'none'}")
    return 0


def select(affected_packages: list[str], selection: list[str], packages: list[str]) -> list[str]:
    """The `-p` options for the affected packages within a part of the workspace that cargo's `-p` or `--exclude`
    options name."""
    pairs = list(zip(selection[::2], selection[1::2]))
    named = {value for flag, value in pairs if flag in ("-p", "--package")}
    excluded = {value for flag, value in pairs if flag == "--exclude"}
    if (len(selection) % 2 or not pairs or any(flag not in ("-p", "--package", "--exclude") for flag, _ in pairs)
            or (named and excluded) or (named | excluded) - set(packages)):
        raise Refused(f"a part is `-p` packages or `--exclude` packages of this workspace, not `{' '.join(selection)}`")
    part = named or set(packages) - excluded
    return [arg for package in sorted(set(affected_packages) & part) for arg in ("-p", package)]


@dataclass
class Step:
    what: str
    command: list[str]
    env: dict[str, str] = field(default_factory=dict)
    # The programs it needs that may be missing here: the step is skipped, and said, without them.
    needs: tuple[str, ...] = ()


def check_steps(result: Affected, base: str, windows: bool = sys.platform == "win32") -> list[Step]:
    """Required CI's checks for what `result` affects, in CI's order."""
    python = sys.executable or "python3"
    steps = [Step("the change's whitespace", ["git", "diff", "--check", f"{base}...HEAD"]),
             Step("every tracked file belongs to a package or an area", [python, "scripts/dev.py", "affected", "--check"])]
    if result.packages:
        picked = [arg for package in result.packages for arg in ("-p", package)]
        runner = ["pwsh", "-NoProfile", "-File", "scripts/test-isolated.ps1"] if windows else ["sh", "scripts/test-isolated.sh"]
        steps += [
            Step("formatting", ["cargo", "fmt", "--all", "--check"]),
            Step("no RefCell borrow held across an await", ["cargo", "clippy", *picked, "--lib", "--bins", "--examples",
                 "--locked", "--", "-A", "clippy::all", "-D", "clippy::await_holding_refcell_ref"]),
            Step(f"the ci tests of {', '.join(result.packages)}", [*runner, "--profile", "ci", *picked], {"KUMI_NEXTEST": "1"}),
        ]
    if "remote-script" in result.areas:
        steps += [Step("the Remote Script's tests", [python, "-m", "unittest", "discover", "-s", "remote-script", "-p", "test_*.py"]),
                  Step("the Remote Script compiles", [python, "-m", "compileall", "-q", "remote-script/AbletonMcpBridge"])]
    if "live-extension" in result.areas:
        steps += [Step("the Live extension's packages", ["npm", "ci", "--prefix", "apps/live-extension", "--ignore-scripts"], needs=("npm",)),
                  Step("the Live extension's tests", ["npm", "test", "--prefix", "apps/live-extension"], needs=("npm",))]
    if "renoise-tool" in result.areas:
        steps.append(Step("the Renoise tool's tests", ["sh", "daws/renoise/tests/run.sh"], needs=("luajit",)))
    steps += [Step("the release scripts' tests", [python, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*release.py"]),
              Step("dev.py's tests", [python, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_dev.py"])]
    if not windows:
        steps.append(Step("the installers' syntax", ["sh", "scripts/check-installers.sh", "."], needs=("shellcheck", "pwsh")))
    return steps


def command_check(args: argparse.Namespace) -> int:
    result = affected(changes_since(args.base), Workspace.read(), load_areas())
    steps = check_steps(result, args.base)
    if result.packages and not args.dry_run:
        if subprocess.run(["cargo", "nextest", "--version"], capture_output=True).returncode:
            raise Refused("cargo-nextest isn't installed, and the tests run on it as CI's do: cargo install cargo-nextest --locked")
    print(f"{result.files} changed file{'' if result.files == 1 else 's'} since {args.base}; packages: "
          f"{', '.join(result.packages) or 'none'}; areas: {', '.join(result.areas) or 'none'}")
    for step in steps:
        shown = " ".join([*(f"{key}={value}" for key, value in step.env.items()), *step.command])
        missing = [program for program in step.needs if not shutil.which(program)]
        if missing:
            print(f"-- {step.what}: skipped, without {' and '.join(missing)} on PATH")
            continue
        print(f"-- {step.what}: {shown}", flush=True)
        if args.dry_run:
            continue
        if subprocess.run(step.command, cwd=ROOT, env={**os.environ, **step.env}).returncode:
            print(f"dev.py check: {step.what} failed", file=sys.stderr)
            return 1
    print("dev.py check: every check passed." if not args.dry_run else "dev.py check: nothing run (--dry-run).")
    return 0


def command_select(args: argparse.Namespace) -> int:
    picked = select(args.affected.split(), args.selection, sorted(Workspace.read().folders))
    if picked:
        print(" ".join(picked))
    return 0


def command_areas(args: argparse.Namespace) -> int:
    areas = load_areas()
    if args.within and args.within not in {area.name for area in areas}:
        raise Refused(f"there's no area {args.within} in scripts/areas.toml")
    outside = []
    for path in args.paths:
        names = [area.name for area in areas if area.holds(path)]
        print(f"{path}: {', '.join(names) or 'no area'}")
        if args.within and args.within not in names:
            outside.append(path)
    if outside:
        print(f"Not in {args.within}: {', '.join(outside)}", file=sys.stderr)
        return 1
    return 0


@dataclass
class Rewrite:
    """A path that moved: a repository path (`[[move]]` in scripts/moves.toml) or a Rust path (`[[rust]]`)."""
    old: str
    new: str
    rust: bool = False

    def apply(self, line: str) -> str:
        """`line` with each place it names the old path naming the new one."""
        out, at = [], 0
        while (found := line.find(self.old, at)) >= 0:
            end = found + len(self.old)
            out += [line[at:found], self.new if self.names(line, found, end) else self.old]
            at = end
        return "".join(out) + line[at:]

    def names(self, line: str, start: int, end: int) -> bool:
        """Whether line[start:end] is the old path itself, not a piece of a longer name or path."""
        before, after = line[start - 1:start], line[end:end + 1]
        if self.rust:
            return before not in WORD and after not in WORD
        # `./crates/x` and `/crates/x` (`concat!(env!("KUMI_WORKSPACE"), "/crates/x")`) name it from the checkout's
        # root; `../crates/x` and `a/crates/x` don't.
        lead = line[:start]
        rooted = lead.endswith("./") and lead[-3:-2] not in PATH or lead.endswith("/") and lead[-2:-1] not in PATH
        if before in PATH and not rooted:
            return False
        # A one-word path (`protocol`) is often a word in prose too: it moves only as a folder of a longer path.
        return after == "/" if "/" not in self.old else after not in WORD and after != "-"

    def moved(self, path: str) -> str:
        """Where a repository path is after this move."""
        if path == self.old or path.startswith(self.old + "/"):
            return self.new + path[len(self.old):]
        return path


def load_moves(text: str) -> list[Rewrite]:
    table = tomllib.loads(text)
    if set(table) - {"move", "rust"}:
        raise Refused(f"{MOVES} holds [[move]] and [[rust]] entries only, not {', '.join(sorted(set(table) - {'move', 'rust'}))}")
    rewrites = []
    for kind in ("move", "rust"):
        for entry in table.get(kind, []):
            if set(entry) != {"from", "to"} or not all(isinstance(value, str) and value for value in entry.values()):
                raise Refused(f"{MOVES}: each [[{kind}]] has a from and a to, and nothing else: {entry}")
            rewrites.append(Rewrite(entry["from"], entry["to"], rust=kind == "rust"))
    return rewrites


def moved_to(path: str, rewrites: list[Rewrite]) -> str:
    """Where a repository path is after every move in the map, in order."""
    for rewrite in rewrites:
        if not rewrite.rust:
            path = rewrite.moved(path)
    return path


def git(repo: Path, *args: str) -> str:
    try:
        return subprocess.run(["git", "-c", "core.quotePath=false", *args], cwd=repo, capture_output=True, text=True,
                              check=True).stdout
    except subprocess.CalledProcessError as failed:
        raise Refused(f"git {' '.join(args)} failed: {failed.stderr.strip()}") from None


def added_lines(diff: str) -> dict[str, list[int]]:
    """The lines (from 1) each file has that the other side hasn't, from `git diff -U0`; none for a file it deletes."""
    lines: dict[str, list[int]] = {}
    path, header = None, False
    for line in diff.splitlines():
        if line.startswith("diff --git "):
            path, header = None, True
        elif header and line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("@@ "):
            header = False
            hunk = re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", line)
            if path and hunk:
                start = int(hunk[1])
                lines.setdefault(path, []).extend(range(start, start + int(hunk[2] or 1)))
    return lines


def rewrite_added(repo: Path, base: str, rewrites: list[Rewrite]) -> dict[str, list[int]]:
    """Rewrites the moved paths in the lines the branch adds since `base`, in the working tree: the lines it changed,
    by file."""
    changed = {}
    diff = git(repo, "diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", f"{base}...HEAD")
    for path, numbers in added_lines(diff).items():
        try:
            with open(repo / path, encoding="utf-8", newline="") as file:
                lines = file.read().splitlines(keepends=True)
        except (OSError, UnicodeDecodeError):
            continue
        hit = []
        for number in numbers:
            if number <= len(lines):
                line = lines[number - 1]
                for rewrite in rewrites:
                    line = rewrite.apply(line)
                if line != lines[number - 1]:
                    lines[number - 1] = line
                    hit.append(number)
        if hit:
            with open(repo / path, "w", encoding="utf-8", newline="") as file:
                file.write("".join(lines))
            changed[path] = hit
    return changed


def rebase_moves(repo: Path, onto: str, merge: bool = False, build: bool = True) -> int:
    """Brings the branch checked out in `repo` onto `onto` across the moves in `onto`'s map (see the module's
    docstring). 0 when it's there and builds; 1 with what's left to do."""
    try:
        rewrites = load_moves(git(repo, "show", f"{onto}:{MOVES}"))
    except Refused as refused:
        raise Refused(f"no {MOVES} at {onto}, so nothing has moved there ({refused})") from None
    git_dir = Path(git(repo, "rev-parse", "--absolute-git-dir").strip())
    if any((git_dir / name).exists() for name in ("rebase-merge", "rebase-apply", "MERGE_HEAD")):
        raise Refused("a rebase or merge is under way here: finish it (git rebase --continue, or git commit) and run this again")
    left = 0
    if subprocess.run(["git", "merge-base", "--is-ancestor", onto, "HEAD"], cwd=repo).returncode:
        if git(repo, "status", "--porcelain", "--untracked-files=no").strip():
            raise Refused("the working tree has changes: commit them first")
        command = ["git", "-c", "merge.directoryRenames=true", "-c", "merge.renameLimit=100000",
                   *(["merge", "--no-edit", onto] if merge else ["rebase", onto])]
        print(f"-- {' '.join(command)}", flush=True)
        done = subprocess.run(command, cwd=repo, capture_output=True, text=True)
        if (done.stdout + done.stderr).strip():
            print((done.stdout + done.stderr).rstrip())
        if done.returncode:
            conflicts = git(repo, "diff", "--name-only", "--diff-filter=U").splitlines()
            print(f"-- {len(conflicts)} file{'' if len(conflicts) == 1 else 's'} in conflict:" if conflicts else
                  f"-- git {'merge' if merge else 'rebase'} stopped (above)")
            for path in conflicts:
                print(f"  {path}")
            print(f"Resolve, then {'git commit' if merge else 'git rebase --continue (until it ends)'}, and run "
                  "rebase-moves again: it rewrites the moved paths and builds.")
            return 1
    else:
        print(f"-- HEAD already has {onto}")
    changed = rewrite_added(repo, onto, rewrites)
    lines = sum(len(numbers) for numbers in changed.values())
    print(f"-- moved paths rewritten in the lines the branch adds: {lines} line{'' if lines == 1 else 's'}"
          + (" (review them with git diff, then commit)" if changed else ""))
    for path, numbers in sorted(changed.items()):
        print(f"  {path}: {', '.join(map(str, numbers))}")
    added = git(repo, "diff", "--name-only", "--diff-filter=A", f"{onto}...HEAD").splitlines()
    behind = [(path, moved_to(path, rewrites)) for path in added if moved_to(path, rewrites) != path]
    if behind:
        left += 1
        print("-- files the branch adds where a path moved away: move each with git mv")
        for path, where in behind:
            print(f"  {path} -> {where}")
    if build:
        if not shutil.which("cargo"):
            print("-- the build: skipped, without cargo on PATH")
        else:
            print("-- cargo check --workspace --all-targets --locked", flush=True)
            done = subprocess.run(["cargo", "check", "--workspace", "--all-targets", "--locked", "--message-format", "short"],
                                  cwd=repo, capture_output=True, text=True)
            errors = [line for line in done.stderr.splitlines() if re.search(r"(^|: )error(\[\w+\])?: ", line)
                      and "could not compile" not in line]
            if done.returncode:
                left += 1
                print(f"  {len(errors)} error{'' if len(errors) == 1 else 's'}" + (":" if errors else ""))
                for line in errors[:40] if errors else done.stderr.strip().splitlines()[-20:]:
                    print(f"  {line}")
            else:
                print("  it builds")
    print("rebase-moves: done." if not left else "rebase-moves: the branch is on its new base, with the above left to fix.")
    return 1 if left else 0


def command_rebase_moves(args: argparse.Namespace) -> int:
    return rebase_moves(Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip()), args.onto, args.merge, not args.no_build)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="dev.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    what = commands.add_parser("affected", help="the packages, areas and CI jobs a change affects")
    what.add_argument("--base", help="the ref the change is measured from (its merge base with HEAD); none: everything")
    what.add_argument("--changes", metavar="FILE", help="read `git diff --name-status` lines from FILE (- for stdin)")
    what.add_argument("--json", action="store_true", help="print JSON")
    what.add_argument("--job", choices=JOBS, help="print only whether this job runs: true or false")
    what.add_argument("--github-output", action="store_true", help="write each job, and the packages and areas, to $GITHUB_OUTPUT")
    what.add_argument("--check", action="store_true", help="fail, naming them, on tracked files no package or area holds")
    what.set_defaults(run=command_affected)
    where = commands.add_parser("areas", help="the areas each path belongs to")
    where.add_argument("--in", dest="within", metavar="AREA", help="fail, naming them, on paths outside AREA")
    where.add_argument("paths", nargs="+")
    where.set_defaults(run=command_areas)
    check = commands.add_parser("check", help="Required CI's checks for what this branch changes")
    check.add_argument("--base", default="origin/main", help="the ref the branch is measured from (default origin/main)")
    check.add_argument("--dry-run", action="store_true", help="say what would run, and run nothing")
    check.set_defaults(run=command_check)
    part = commands.add_parser("select", help="a CI part's affected packages, as -p options")
    part.add_argument("affected", help="the affected packages, space-separated")
    part.add_argument("selection", nargs=argparse.REMAINDER, help="the part: -p NAME... or --exclude NAME...")
    part.set_defaults(run=command_select)
    rebase = commands.add_parser("rebase-moves", help="bring a branch from before a move onto main, moved paths rewritten")
    rebase.add_argument("--onto", default="origin/main", help="the ref to rebase onto, whose moves map is read (default origin/main)")
    rebase.add_argument("--merge", action="store_true", help="merge --onto into the branch instead of rebasing it")
    rebase.add_argument("--no-build", action="store_true", help="don't run cargo check afterwards")
    rebase.set_defaults(run=command_rebase_moves)
    args = parser.parse_args(argv)
    try:
        return args.run(args)
    except Refused as refused:
        print(f"dev.py: {refused}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
