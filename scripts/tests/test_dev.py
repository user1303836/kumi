"""scripts/dev.py: globs, packages and their dependents, and what a change affects, on a made-up workspace and on this
checkout's own (git and cargo read it); and rebase-moves, on repositories of its own.
"""
import ast
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("dev_script", ROOT / "scripts/dev.py")
dev = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = dev  # dataclasses look their module up there
SPEC.loader.exec_module(dev)


def metadata(packages: dict[str, tuple[str, list[tuple[str, str | None]]]]) -> dict:
    """`cargo metadata --no-deps` for packages {name: (folder, [(dependency, kind)])} under /w."""
    folders = {name: folder for name, (folder, _) in packages.items()}
    return {
        "workspace_root": "/w",
        "packages": [
            {"name": name, "manifest_path": f"/w/{folder}/Cargo.toml",
             "dependencies": [{"name": used, "kind": kind, "path": f"/w/{folders[used]}"} for used, kind in uses]
             + [{"name": "serde", "kind": None}]}
            for name, (folder, uses) in packages.items()
        ],
    }


# store ← runtime ← app; the host is only a dev-dependency of the runtime and the app; the extension links the wire.
WORKSPACE = dev.Workspace.from_metadata(metadata({
    "store": ("crates/store", []),
    "wire": ("crates/wire", []),
    "host": ("crates/host", [("wire", None)]),
    "runtime": ("crates/runtime", [("store", None), ("wire", None), ("host", "dev")]),
    "app": ("crates/app", [("runtime", None), ("host", "dev")]),
    "reaper-kumi": ("crates/extension", [("wire", None)]),
    "nested": ("crates/runtime/tests/nested", []),
}))
AREAS = [
    dev.Area("docs", [dev.glob(p) for p in ("docs/**", "**/*.md", "LICENSE*")]),
    dev.Area("workspace", [dev.glob("Cargo.lock")], feeds=["*"]),
    dev.Area("protocol", [dev.glob("protocol/**")], feeds=["runtime"]),
    dev.Area("renoise-tool", [dev.glob("daws/renoise/**")]),
    dev.Area("renoise-tool-read", [dev.glob("daws/renoise/tool/version.lua")], feeds=["app"],
             layout=[dev.glob("daws/renoise/tool/**")]),
    dev.Area("reaper-extension", [dev.glob("crates/extension/**")]),
    dev.Area("install", [dev.glob("install.sh"), dev.glob("crates/app/src/install/**")],
             unless=[dev.glob("**/tests.rs")]),
    dev.Area("network", [dev.glob("crates/runtime/src/net/**")]),
    dev.Area("workflows", [dev.glob(".github/**")]),
    dev.Area("ci", [dev.glob(".github/workflows/ci.yml")]),
]


def run(*changes: str) -> "dev.Affected":
    """What `changes` (`git diff --name-status` lines, tab-separated) affect in the made-up workspace."""
    return dev.affected(dev.parse_name_status("\n".join(changes)), WORKSPACE, AREAS)


def jobs(result: "dev.Affected") -> set[str]:
    return {job for job, runs in result.jobs.items() if runs}


RUST = {"rust_linux", "rust_lint", "rust_platforms"}
CODE = {"remote_script", "live_extension"}


class Globs(unittest.TestCase):
    def test_a_star_stays_inside_a_folder_and_two_cross_folders(self):
        self.assertTrue(dev.glob("scripts/*.py").match("scripts/release.py"))
        self.assertFalse(dev.glob("scripts/*.py").match("scripts/tests/test_release.py"))
        self.assertTrue(dev.glob("scripts/**").match("scripts/tests/test_release.py"))
        self.assertTrue(dev.glob("**/*.md").match("README.md"))
        self.assertTrue(dev.glob("**/*.md").match("crates/kumi/README.md"))
        self.assertFalse(dev.glob("**/*.md").match("crates/kumi/README.md.bak"))
        self.assertTrue(dev.glob("crates/a/src/lifecycle*").match("crates/a/src/lifecycle_cli.rs"))
        self.assertFalse(dev.glob("crates/a/src/lifecycle*").match("crates/a/src/lifecycle/mod.rs"))
        self.assertTrue(dev.glob("src/?.rs").match("src/a.rs"))
        self.assertFalse(dev.glob("a.b").match("axb"), "a dot is a dot")


class Packages(unittest.TestCase):
    def test_a_file_belongs_to_the_deepest_package_folder_that_holds_it(self):
        self.assertEqual(WORKSPACE.package_of("crates/runtime/src/lib.rs"), "runtime")
        self.assertEqual(WORKSPACE.package_of("crates/runtime/tests/nested/src/lib.rs"), "nested")
        self.assertIsNone(WORKSPACE.package_of("crates/runtimes/x.rs"), "a folder's name, not a prefix of it")
        self.assertIsNone(WORKSPACE.package_of("install.sh"))

    def test_dependents_carry_a_change_on_and_a_dev_dependent_only_sees_it(self):
        self.assertEqual(WORKSPACE.dependents({"store"}), {"store", "runtime", "app"})
        self.assertEqual(WORKSPACE.dependents({"wire"}), {"wire", "host", "runtime", "app", "reaper-kumi"})
        # The host's change reaches the runtime's and the app's tests, not what depends on the runtime's library.
        self.assertEqual(WORKSPACE.dependents({"host"}), {"host", "runtime", "app"})
        self.assertEqual(WORKSPACE.dependents({"app"}), {"app"})


class Affected(unittest.TestCase):
    def test_docs_alone_build_and_test_nothing_even_in_a_packages_folder(self):
        result = run("M\tdocs/en/TESTING.md", "M\tcrates/runtime/README.md", "M\tLICENSE.md")
        self.assertEqual((result.packages, jobs(result)), ([], set()))
        self.assertEqual(result.areas, ["docs"])

    def test_a_packages_change_affects_it_and_its_dependents(self):
        result = run("M\tcrates/store/src/lib.rs")
        self.assertEqual(result.packages, ["app", "runtime", "store"])
        self.assertEqual(jobs(result), RUST | CODE)

    def test_an_area_feeds_its_packages_and_every_package_for_a_star(self):
        self.assertEqual(run("M\tprotocol/ops.json").packages, ["app", "runtime"])
        self.assertEqual(run("M\tCargo.lock").packages, sorted(WORKSPACE.folders))
        self.assertIn("reaper_extension", jobs(run("M\tCargo.lock")), "the extension builds with the lockfile")

    def test_the_renoise_tool_runs_rust_only_for_what_kumi_reads_of_it(self):
        lua = run("M\tdaws/renoise/tool/payload/notes.lua")
        self.assertEqual(jobs(lua), CODE | {"renoise_tool"})
        self.assertEqual(lua.packages, [])
        for change in ("M\tdaws/renoise/tool/version.lua", "A\tdaws/renoise/tool/payload/new.lua",
                       "D\tdaws/renoise/tool/payload/old.lua", "R100\tdaws/renoise/tool/a.lua\tdaws/renoise/tool/b.lua"):
            result = run(change)
            self.assertEqual(jobs(result), RUST | CODE | {"renoise_tool"}, change)
            self.assertEqual(result.packages, ["app"], change)
        self.assertEqual(jobs(run("M\tdaws/renoise/tests/run.sh")), CODE | {"renoise_tool"})

    def test_reaper_s_extension_runs_for_its_crate_what_it_links_and_ci(self):
        self.assertIn("reaper_extension", jobs(run("M\tcrates/extension/src/lib.rs")))
        self.assertIn("reaper_extension", jobs(run("M\tcrates/wire/src/frame.rs")))
        self.assertNotIn("reaper_extension", jobs(run("M\tcrates/store/src/lib.rs")))

    def test_ci_s_workflow_runs_every_ci_job_and_another_workflow_the_broad_ones(self):
        self.assertEqual(jobs(run("M\t.github/workflows/ci.yml")), RUST | CODE | {"reaper_extension", "renoise_tool"})
        self.assertEqual(jobs(run("M\t.github/workflows/extended.yml")), RUST | CODE)

    def test_a_change_no_package_holds_and_no_area_feeds_affects_every_package(self):
        self.assertEqual(run("M\t.github/workflows/extended.yml").packages, sorted(WORKSPACE.folders))
        self.assertEqual(run("M\tinstall.sh").packages, sorted(WORKSPACE.folders))
        self.assertEqual(run("M\tcrates/store/src/lib.rs", "M\tprotocol/ops.json").packages, ["app", "runtime", "store"])
        self.assertEqual(run("M\tdaws/renoise/tool/payload/notes.lua").packages, [], "the tool's own Lua runs no Rust")

    def test_the_install_surface_and_network_are_their_own_jobs(self):
        self.assertIn("installer", jobs(run("M\tinstall.sh")))
        self.assertIn("installer", jobs(run("M\tcrates/app/src/install/swap.rs")))
        self.assertNotIn("installer", jobs(run("M\tcrates/app/src/install/tests.rs")), "its tests aren't")
        self.assertIn("network", jobs(run("M\tcrates/runtime/src/net/proxy.rs")))
        self.assertNotIn("network", jobs(run("M\tcrates/runtime/src/lib.rs")))

    def test_a_rename_counts_both_its_paths(self):
        result = run("R090\tcrates/app/src/install/old.rs\tcrates/app/src/old.rs")
        self.assertIn("installer", jobs(result), "moving a file out of the install surface changes it")

    def test_no_change_runs_nothing_and_no_base_everything(self):
        self.assertEqual(jobs(run()), set())
        everything = dev.affected(None, WORKSPACE, AREAS)
        self.assertEqual(set(everything.packages), set(WORKSPACE.folders))
        self.assertEqual(jobs(everything), set(dev.JOBS))

    def test_feeding_a_package_the_workspace_hasnt_got_is_refused(self):
        areas = [dev.Area("odd", [dev.glob("odd/**")], feeds=["gone"])]
        with self.assertRaises(dev.Refused):
            dev.affected(dev.parse_name_status("M\todd/x"), WORKSPACE, areas)


class Select(unittest.TestCase):
    PACKAGES = sorted(WORKSPACE.folders)

    def test_a_part_tests_its_affected_packages_only(self):
        self.assertEqual(dev.select(["store", "runtime", "app"], ["--exclude", "wire", "--exclude", "host"], self.PACKAGES),
                         ["-p", "app", "-p", "runtime", "-p", "store"])
        self.assertEqual(dev.select(["store", "runtime", "app"], ["-p", "wire", "-p", "host"], self.PACKAGES), [])
        self.assertEqual(dev.select(["wire", "app"], ["-p", "wire", "-p", "host"], self.PACKAGES), ["-p", "wire"])

    def test_a_part_names_this_workspaces_packages_one_way(self):
        for selection in (["-p", "gone"], ["-p", "wire", "--exclude", "host"], ["-p"], ["--all"]):
            with self.assertRaises(dev.Refused, msg=selection):
                dev.select(["wire"], selection, self.PACKAGES)

    def test_no_part_named_is_the_whole_workspace(self):
        self.assertEqual(dev.select(["wire", "app"], [], self.PACKAGES), ["-p", "app", "-p", "wire"])

    def test_a_part_tested_by_a_profile_is_its_packages_that_profile_builds(self):
        # The platform jobs' `ci-os` set: the packages whose `it` binary it takes in, not those only its `& not (…)` names.
        ci_os = """
(
    (binary_id(app::it) & test(/^(install|cli)::/))
  | (binary_id(wire::it) & test(/^system::/))
) & not (
    (binary_id(store::it) & test(/^slow::/))
  | (binary_id(app::it) & test(/^long::/))
)
"""
        self.assertEqual(dev.filter_packages(ci_os), {"app", "wire"})
        self.assertEqual(dev.filter_packages("binary_id(host::it) | binary_id(wire::it)"), {"host", "wire"})
        tested = dev.filter_packages(ci_os)
        self.assertEqual(dev.select(["app", "store", "wire"], ["--exclude", "app"], self.PACKAGES, tested), ["-p", "wire"])
        self.assertEqual(dev.select(["store", "runtime"], [], self.PACKAGES, tested), [])

    def test_this_checkouts_platform_set_takes_in_the_app_and_not_what_it_leaves_out(self):
        tested = dev.profile_packages("ci-os")
        self.assertTrue({"kumi", "ableton-mcp-server", "daw-bridge"} <= tested, tested)
        self.assertFalse({"kumi-listening", "kumi-web"} & tested, tested)
        with self.assertRaises(dev.Refused):
            dev.profile_packages("no-such-profile")


class Check(unittest.TestCase):
    def steps(self, *changes: str, windows: bool = False) -> list[str]:
        return [" ".join(step.command) for step in dev.check_steps(run(*changes), "origin/main", windows=windows)]

    def test_a_packages_change_tests_it_and_its_dependents_only(self):
        steps = self.steps("M\tcrates/store/src/lib.rs")
        self.assertIn("cargo fmt --all --check", steps)
        self.assertIn("sh scripts/test-isolated.sh --profile ci -p app -p runtime -p store", steps)
        self.assertTrue(any(step.startswith("cargo clippy -p app -p runtime -p store --lib --bins --examples") for step in steps), steps)
        self.assertFalse(any("remote-script" in step or "renoise" in step for step in steps), steps)

    def test_docs_alone_run_no_rust(self):
        steps = self.steps("M\tdocs/en/TESTING.md")
        self.assertFalse(any(step.startswith("cargo") or "test-isolated" in step for step in steps), steps)
        self.assertTrue(any("test_*release.py" in step for step in steps), "the release scripts' tests check the docs' links")

    def test_each_area_runs_its_own_checks(self):
        self.assertTrue(any("daws/renoise/tests/run.sh" in step for step in self.steps("M\tdaws/renoise/tool/payload/notes.lua")))
        self.assertTrue(any("pwsh" in step and "--profile ci -p" in step for step in self.steps("M\tcrates/store/src/lib.rs", windows=True)))


# The layers as Kumi's crates keep them: the app on the DAWs' integrations, the integrations on kumi-daw, the agent and
# daws/own/, kumi-daw on the agent, the bridges on kumi-common and each other.
LAYERED = {
    "kumi": ("crates/kumi", [("kumi-agent", None), ("kumi-daw", None), ("kumi-daw-reaper", None), ("daw-bridge", None), ("kumi-testkit", "dev")]),
    "kumi-daw": ("crates/kumi-daw", [("kumi-agent", None), ("kumi-types", None)]),
    "kumi-agent": ("crates/kumi-agent", [("kumi-ai", None), ("kumi-types", None), ("daw-mcp-server", "dev")]),
    "kumi-ai": ("crates/kumi-ai", [("kumi-net", None)]),
    "kumi-web": ("crates/kumi-web", []),
    "kumi-net": ("crates/kumi-net", [("kumi-types", None)]),
    "kumi-types": ("crates/kumi-types", [("kumi-common", None)]),
    "kumi-common": ("crates/kumi-common", [("kumi-testkit", "dev")]),
    "kumi-testkit": ("crates/kumi-testkit", []),
    "kumi-daw-own": ("daws/own/kumi-daw-own", [("kumi-agent", None), ("kumi-daw", None), ("daw-bridge", None)]),
    "kumi-daw-reaper": ("daws/reaper/kumi-daw-reaper", [("kumi-daw-own", None), ("daw-mcp-server", "dev")]),
    "kumi-daw-renoise": ("daws/renoise/kumi-daw-renoise", [("kumi-daw-own", None)]),
    "reaper-kumi": ("daws/reaper/reaper-kumi", [("daw-bridge", None), ("kumi-testkit", "dev")]),
    "daw-mcp-server": ("daws/own/daw-mcp-server", [("daw-bridge", None), ("kumi-common", None)]),
    "daw-bridge": ("daws/own/daw-bridge", []),
}


class Layers(unittest.TestCase):
    def breaks(self, **extra: list[tuple[str, str | None]]) -> list[str]:
        packages = {name: (folder, list(uses) + extra.get(name.replace("-", "_"), [])) for name, (folder, uses) in LAYERED.items()}
        return dev.layer_breaks(dev.Workspace.from_metadata(metadata(packages)))

    def test_the_crates_as_they_are_keep_their_layers(self):
        self.assertEqual(self.breaks(), [])

    def test_a_crate_uses_no_crate_above_it_but_one_of_its_layer(self):
        self.assertEqual(self.breaks(kumi_web=[("kumi-ai", None)]), [])
        [refused] = self.breaks(kumi_types=[("kumi-net", None)])
        self.assertIn("kumi-types -> kumi-net: L0 uses L1", refused)
        # an integration (L5) is above kumi-daw (L4), which is above the agent (L3), and below the app (L6)
        self.assertIn("kumi-agent -> kumi-daw-reaper: L3 uses L5", " ".join(self.breaks(kumi_agent=[("kumi-daw-reaper", None)])))
        self.assertIn("kumi-daw -> kumi-daw-reaper: L4 uses L5", " ".join(self.breaks(kumi_daw=[("kumi-daw-reaper", None)])))
        self.assertIn("kumi-agent -> kumi-daw: L3 uses L4", " ".join(self.breaks(kumi_agent=[("kumi-daw", None)])))
        # a test may use what it likes of the crates
        self.assertEqual(self.breaks(kumi_types=[("kumi-net", "dev")]), [])

    def test_only_the_app_and_tests_of_daws_own_reach_into_daws(self):
        self.assertIn("kumi-agent -> daw-bridge: nothing in crates/ uses daws/", " ".join(self.breaks(kumi_agent=[("daw-bridge", None)])))
        self.assertIn("kumi-agent -> reaper-kumi (dev)", " ".join(self.breaks(kumi_agent=[("reaper-kumi", "dev")])))
        self.assertEqual(self.breaks(kumi_ai=[("daw-bridge", "dev")]), [])
        self.assertIn("kumi -> reaper-kumi", " ".join(self.breaks(kumi=[("reaper-kumi", None)])))

    def test_a_daws_crates_use_no_other_daws(self):
        [refused] = self.breaks(kumi_daw_reaper=[("kumi-daw-renoise", None)])
        self.assertIn("kumi-daw-reaper -> kumi-daw-renoise: a DAW's crates use no other DAW's", refused)
        self.assertTrue(self.breaks(kumi_daw_renoise=[("reaper-kumi", "dev")]))

    def test_a_bridge_uses_nothing_of_kumis_crates_but_kumi_common(self):
        self.assertIn("reaper-kumi -> kumi-types: a bridge uses nothing", " ".join(self.breaks(reaper_kumi=[("kumi-types", None)])))
        self.assertIn("daw-mcp-server -> kumi-daw-own", " ".join(self.breaks(daw_mcp_server=[("kumi-daw-own", None)])))

    def test_the_tests_support_is_a_dev_dependency_only_and_a_new_crate_needs_a_layer(self):
        self.assertIn("kumi-net -> kumi-testkit: kumi-testkit is the tests' support", " ".join(self.breaks(kumi_net=[("kumi-testkit", None)])))
        packages = dict(LAYERED, **{"kumi-new": ("crates/kumi-new", [])})
        [refused] = dev.layer_breaks(dev.Workspace.from_metadata(metadata(packages)))
        self.assertIn("kumi-new (crates/kumi-new) has no layer", refused)


class ThisCheckout(unittest.TestCase):
    """scripts/areas.toml against this checkout's files and packages."""

    @classmethod
    def setUpClass(cls):
        cls.areas = dev.load_areas()
        cls.workspace = dev.Workspace.read()

    def area(self, name: str) -> "dev.Area":
        return next(area for area in self.areas if area.name == name)

    def test_every_tracked_file_belongs_to_a_package_or_an_area(self):
        self.assertEqual(dev.unplaced(dev.tracked(), self.workspace, self.areas), [])

    def test_the_crates_keep_their_layers(self):
        self.assertEqual(dev.layer_breaks(self.workspace), [])

    def test_every_area_feeds_packages_the_workspace_has(self):
        for area in self.areas:
            self.assertLessEqual(set(area.feeds) - {"*"}, set(self.workspace.folders), area.name)

    def test_the_install_surface_holds_the_installs_files_and_not_their_tests_or_the_cli(self):
        install = self.area("install")
        for path in ("install.sh", "crates/kumi/src/install/migration.rs", "daws/ableton/ableton-mcp-server/src/lifecycle/cli.rs",
                     "daws/reaper/reaper-kumi/shim/csurf.cpp", "vendor/willington/README.md", ".github/workflows/installer.yml"):
            self.assertTrue(install.holds(path), path)
        for path in ("crates/kumi/src/cli/native/session.rs", "scripts/tests/daw_bridges.py", "scripts/test-isolated.sh",
                     "daws/own/daw-bridge/tests/it/system.rs", "daws/ableton/kumi-daw-ableton/src/app/willington.rs", "Cargo.lock",
                     "daws/ableton/ableton-mcp-server/src/lifecycle/diagnostics.rs"):
            self.assertFalse(install.holds(path), path)

    def test_the_extended_only_oracles_are_oracles_here(self):
        listed = [line for line in (ROOT / dev.EXTENDED_ONLY).read_text(encoding="utf-8").splitlines() if line and not line.startswith("#")]
        self.assertEqual(listed, sorted(set(listed)))
        tracked = set(dev.tracked())  # with what a lean worktree leaves out
        for oracle in listed:
            self.assertIn(f"{dev.ORACLES}/{oracle}", tracked)

    def test_the_moves_map_names_paths_that_moved(self):
        rewrites = dev.load_moves((ROOT / dev.MOVES).read_text(encoding="utf-8"))
        tracked = dev.tracked()
        for at, move in enumerate(rewrites):
            if move.rust:
                continue
            gone, there = move.old, dev.moved_to(move.new, rewrites[at + 1:])
            self.assertFalse(any(path == gone or path.startswith(gone + "/") for path in tracked), f"{gone} still holds files")
            self.assertTrue(any(path == there or path.startswith(there + "/") for path in tracked), f"{there} holds none")

    def test_what_it_prints_is_utf8_whatever_the_streams_encoding(self):
        # PYTHONIOENCODING=cp1252 stands for a pipe on Windows, which can't encode the arrow.
        done = subprocess.run([sys.executable, str(ROOT / "scripts/dev.py"), "areas", "docs/→.md"], capture_output=True,
                              env={**os.environ, "PYTHONIOENCODING": "cp1252"})
        self.assertEqual(done.returncode, 0, done.stderr.decode("utf-8", "replace"))
        self.assertTrue(done.stdout.decode("utf-8").startswith("docs/→.md: "), done.stdout)

    def test_every_command_it_reads_as_text_is_read_as_utf8(self):
        # text=True alone reads in the system's encoding: cp1252 on Windows, where git's UTF-8 fails to decode. CI runs
        # these tests on Linux, whose UTF-8 hides that.
        tree = ast.parse((ROOT / "scripts/dev.py").read_text(encoding="utf-8"))
        unread = sorted(call.lineno for call in ast.walk(tree) if isinstance(call, ast.Call)
                        and ast.unparse(call.func) == "subprocess.run"
                        and {keyword.arg for keyword in call.keywords} & {"text", "universal_newlines"}
                        and "encoding" not in {keyword.arg for keyword in call.keywords})
        self.assertEqual(unread, [], 'subprocess.run on these lines reads text without encoding="utf-8", errors="replace"')


class Moves(unittest.TestCase):
    def test_a_moved_folder_is_rewritten_where_a_path_names_it_from_the_root(self):
        move = dev.Rewrite("crates/bridge", "daws/live/bridge")
        self.assertEqual(move.apply("cd crates/bridge && cargo test -p bridge"), "cd daws/live/bridge && cargo test -p bridge")
        self.assertEqual(move.apply("`crates/bridge/src/lib.rs`, ./crates/bridge."), "`daws/live/bridge/src/lib.rs`, ./daws/live/bridge.")
        self.assertEqual(move.apply('concat!(env!("KUMI_WORKSPACE"), "/crates/bridge/x")'),
                         'concat!(env!("KUMI_WORKSPACE"), "/daws/live/bridge/x")')
        for line in ('path = "../crates/bridge"', "crates/bridge-kit", "crates/bridges", "old/crates/bridge", "daws/live/bridge"):
            self.assertEqual(move.apply(line), line)

    def test_a_one_word_path_moves_only_as_a_folder(self):
        move = dev.Rewrite("protocol", "daws/live/protocol")
        self.assertEqual(move.apply("protocol/ops.json, /protocol/x"), "daws/live/protocol/ops.json, /daws/live/protocol/x")
        for line in ("the wire protocol", '"protocol"', "protocol.json", "daws/live/protocol/ops.json"):
            self.assertEqual(move.apply(line), line)

    def test_a_rust_path_moves_whole_and_never_inside_a_longer_name(self):
        rust = dev.Rewrite("kumi_runtime::core::daw", "kumi_types::daw", rust=True)
        self.assertEqual(rust.apply("use kumi_runtime::core::daw::{Daw, Kind};"), "use kumi_types::daw::{Daw, Kind};")
        self.assertEqual(rust.apply("::kumi_runtime::core::daw::Daw"), "::kumi_types::daw::Daw")
        for line in ("use kumi_runtime::core::daws;", "my_kumi_runtime::core::daw::Daw"):
            self.assertEqual(rust.apply(line), line)

    def test_a_path_from_a_crates_root_moves_only_in_that_crate(self):
        rust = dev.load_moves('[[rust]]\nfrom = "crate::old"\nto = "crate::new"\nin = "daws/x/a"\n')[0]
        self.assertTrue(rust.applies_to("daws/x/a/src/lib.rs"))
        self.assertFalse(rust.applies_to("daws/x/ab/src/lib.rs"))
        self.assertFalse(rust.applies_to("crates/kumi/src/lib.rs"))
        with self.assertRaises(dev.Refused):
            dev.load_moves('[[move]]\nfrom = "a"\nto = "b"\nin = "c"\n')

    def test_moves_follow_one_another_in_order(self):
        rewrites = dev.load_moves('[[move]]\nfrom = "a"\nto = "b/a"\n\n[[move]]\nfrom = "b/a/x.rs"\nto = "b/a/y/x.rs"\n')
        self.assertEqual(dev.moved_to("a/x.rs", rewrites), "b/a/y/x.rs")
        self.assertEqual(dev.moved_to("a/z.rs", rewrites), "b/a/z.rs")
        self.assertEqual(dev.moved_to("ab/x.rs", rewrites), "ab/x.rs")

    def test_the_map_takes_moves_and_rust_paths_each_from_and_to(self):
        for text in ('[[moves]]\nfrom = "a"\nto = "b"\n', '[[move]]\nfrom = "a"\n', '[[rust]]\nfrom = "a"\nto = "b"\nwhy = "c"\n'):
            with self.assertRaises(dev.Refused, msg=text):
                dev.load_moves(text)

    def test_the_lines_a_diff_adds_by_file(self):
        diff = "\n".join(["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -1,0 +2,2 @@ fn", "+a", "+++ b/not-a-file", "@@ -9 +12 @@",
                          "-q", "+r", "@@ -20,2 +22,0 @@", "-gone", "-gone", "diff --git a/y b/y", "deleted file mode 100644",
                          "--- a/y", "+++ /dev/null", "@@ -1 +0,0 @@", "-y"])
        self.assertEqual(dev.added_lines(diff), {"x": [2, 3, 12]})


class Oracles(unittest.TestCase):
    SELECTED = {"bridge::it": {"host_racks::racks_match", "als::reads"}, "bridge": {"host::probes::tests::match"},
                "runtime::it": {"views::views_match"}}

    def test_a_test_modules_tests_are_its_selected_ones_and_other_files_are_unknown(self):
        self.assertEqual(dev.tests_in("daws/live/bridge/tests/it/host_racks.rs", self.SELECTED), ["host_racks::racks_match"])
        self.assertEqual(dev.tests_in("daws/live/bridge/tests/it/host_looper.rs", self.SELECTED), [])
        # host/recovery_tests.rs is `host::recovery::tests` through #[path]: a src file can't say whose tests it holds.
        self.assertIsNone(dev.tests_in("daws/live/bridge/src/host/probes.rs", self.SELECTED))
        self.assertIsNone(dev.tests_in("crates/runtime/tests/support/library.rs", self.SELECTED))

    def test_an_oracle_only_unselected_tests_read_is_extended_only(self):
        sources = {"daws/live/bridge/tests/it/host_racks.rs": 'oracle("bridge/racks-oracle.json")',
                   "daws/live/bridge/tests/it/host_looper.rs": 'oracle("bridge/looper-oracle.json")',
                   "daws/live/bridge/src/host/recovery_tests.rs": 'oracle("bridge/recovery-oracle.json")',
                   "crates/runtime/tests/support/library.rs": 'oracle("runtime/library-oracle.json")',
                   "crates/runtime/tests/it/views.rs": 'oracle("runtime/library-oracle.json")'}
        oracles = ["bridge/racks-oracle.json", "bridge/looper-oracle.json", "bridge/recovery-oracle.json", "runtime/library-oracle.json",
                   "runtime/unread-oracle.json"]
        self.assertEqual(dev.extended_only(oracles, sources, self.SELECTED), ["bridge/looper-oracle.json"])


class RebaseMoves(unittest.TestCase):
    """A branch from before a move, brought across it in a repository of its own."""

    MAP = ('[[move]]\nfrom = "crates/a"\nto = "daws/x/a"\n\n[[rust]]\nfrom = "kumi_runtime::core::daw"\nto = "kumi_types::daw"\n\n'
           '[[rust]]\nfrom = "crate::old"\nto = "crate::new"\nin = "daws/x/a"\n')

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repo = Path(folder.name) / "repo"
        (Path(folder.name) / "gitconfig").write_text("")
        identity = {f"GIT_{who}_{what}": value for who in ("AUTHOR", "COMMITTER")
                    for what, value in (("NAME", "Test"), ("EMAIL", "test@example.com"))}
        environment = mock.patch.dict(os.environ, {**identity, "GIT_CONFIG_GLOBAL": str(Path(folder.name) / "gitconfig"),
                                                   "GIT_CONFIG_NOSYSTEM": "1"})
        environment.start()
        self.addCleanup(environment.stop)
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.write("crates/a/src/lib.rs", "pub fn a() {}\n")
        self.write("README.md", "See crates/a/src/lib.rs.\n\nOne.\n\nBefore the move, crates/a held it.\n")
        self.write("scripts/moves.toml", "")
        self.commit("Start")
        self.git("checkout", "-q", "-b", "feature")
        self.write("crates/a/src/new.rs", "use kumi_runtime::core::daw::Daw;\nuse crate::old::f;\n")
        self.write("crates/a/src/lib.rs", "pub fn a() {}\npub fn b() {}\n")
        self.write("README.md", self.read("README.md") + "\nAlso crates/a/src/new.rs, in protocol and kumi_runtime::core::daws (crate::old).\n")
        self.commit("Feature")
        self.git("checkout", "-q", "main")
        (self.repo / "daws/x").mkdir(parents=True)
        self.git("mv", "crates/a", "daws/x/a")
        self.write("README.md", self.read("README.md").replace("See crates/a", "See daws/x/a"))
        self.write("scripts/moves.toml", self.MAP)
        self.commit("Move")
        self.git("checkout", "-q", "feature")

    def git(self, *args: str) -> str:
        return subprocess.run(["git", *args], cwd=self.repo, capture_output=True, text=True, check=True).stdout

    def write(self, path: str, text: str):
        (self.repo / path).parent.mkdir(parents=True, exist_ok=True)
        (self.repo / path).write_text(text, encoding="utf-8")

    def read(self, path: str) -> str:
        return (self.repo / path).read_text(encoding="utf-8")

    def commit(self, subject: str):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", subject)

    def bring(self, merge: bool = False) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = dev.rebase_moves(self.repo, "main", merge=merge, build=False)
        return code, out.getvalue()

    def test_a_branch_lands_where_its_folder_moved_with_the_lines_it_adds_rewritten(self):
        code, out = self.bring()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.git("log", "--format=%s").split(), ["Feature", "Move", "Start"])
        self.assertFalse((self.repo / "crates").exists(), out)
        self.assertEqual(self.read("daws/x/a/src/lib.rs"), "pub fn a() {}\npub fn b() {}\n")
        self.assertEqual(self.read("daws/x/a/src/new.rs"), "use kumi_types::daw::Daw;\nuse crate::new::f;\n")
        # Main's own lines stay as main has them, history included.
        self.assertEqual(self.read("README.md"), "See daws/x/a/src/lib.rs.\n\nOne.\n\nBefore the move, crates/a held it.\n"
                                                 "\nAlso daws/x/a/src/new.rs, in protocol and kumi_runtime::core::daws (crate::old).\n")
        self.assertIn("README.md: 7", out)
        code, out = self.bring()
        self.assertEqual(code, 0, out)
        self.assertIn("HEAD already has main", out)
        self.assertIn("the lines the branch adds: 0 lines", out, "a second run finds nothing left to do")

    def test_merging_keeps_the_branch_and_names_a_file_added_at_the_old_path_since(self):
        code, out = self.bring(merge=True)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read("daws/x/a/src/new.rs"), "use kumi_types::daw::Daw;\nuse crate::new::f;\n")
        self.commit("Rewritten")
        self.write("crates/a/src/late.rs", "")
        self.commit("Late")
        code, out = self.bring(merge=True)
        self.assertEqual(code, 1, out)
        self.assertIn("crates/a/src/late.rs -> daws/x/a/src/late.rs", out)

    def test_lines_cp1252_cant_decode_are_rewritten_and_a_file_that_isnt_utf8_is_left_as_it_is(self):
        # git's output is read as UTF-8 whatever the system's encoding: Windows' cp1252 can't decode a ” or a kana.
        self.write("README.md", self.read("README.md") + "\n“Quoted” in crates/a/src/lib.rs, かな.\n")
        (self.repo / "latin1.txt").write_bytes(b"caf\xe9 in crates/a\n")
        self.commit("Text")
        code, out = self.bring()
        self.assertEqual(code, 0, out)
        self.assertTrue(self.read("README.md").endswith("\n“Quoted” in daws/x/a/src/lib.rs, かな.\n"), out)
        self.assertEqual((self.repo / "latin1.txt").read_bytes(), b"caf\xe9 in crates/a\n")

    def test_a_conflict_stops_it_and_names_the_file(self):
        self.write("README.md", self.read("README.md").replace("See crates/a/src/lib.rs.", "Read crates/a/src/lib.rs."))
        self.commit("Reworded")
        code, out = self.bring()
        self.assertEqual(code, 1, out)
        self.assertIn("1 file in conflict:\n  README.md", out)
        with self.assertRaises(dev.Refused):
            self.bring()


if __name__ == "__main__":
    unittest.main()
