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
- logic_bridge: Logic Pro's bridge or a package its job tests with it (daw-bridge, daw-mcp-server), what they build
  with, its helper's build script, or CI's workflow;
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
# What CI's Logic job tests on macOS: Logic Pro's bridge and the crates its Logic parts are in.
LOGIC_BRIDGE = ("daw-bridge", "daw-mcp-server", "logic-kumi")
JOBS = ("rust_linux", "rust_lint", "rust_platforms", "remote_script", "live_extension", "reaper_extension",
        "renoise_tool", "logic_bridge", "installer", "network")


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
    code = rust = renoise = reaper = logic = installer = network = unfed = False
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
            logic |= "ci" in names
            installer |= "install" in names
            network |= "network" in names
    unknown = fed - set(workspace.folders)
    if unknown:
        raise Refused(f"areas.toml feeds packages the workspace doesn't have: {', '.join(sorted(unknown))}")
    named = workspace.dependents(owners | fed)
    packages = set(workspace.folders) if unfed else named
    # The extension's own jobs follow what it builds with, never the everything above.
    reaper |= REAPER_EXTENSION in named
    # So do Logic's bridge's (its helper's script feeds the bridge's crate).
    logic |= bool(named & set(LOGIC_BRIDGE))
    jobs = {"rust_linux": rust, "rust_lint": rust, "rust_platforms": rust, "remote_script": code, "live_extension": code,
            "reaper_extension": reaper, "renoise_tool": renoise, "logic_bridge": logic, "installer": installer,
            "network": network}
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
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
    args = parser.parse_args(argv)
    try:
        return args.run(args)
    except Refused as refused:
        print(f"dev.py: {refused}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
