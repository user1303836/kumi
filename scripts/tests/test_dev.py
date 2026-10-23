"""scripts/dev.py: globs, packages and their dependents, and what a change affects, on a made-up workspace and on this
checkout's own (git and cargo read it).
"""
import importlib.util
from pathlib import Path
import sys
import unittest

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

    def test_every_area_feeds_packages_the_workspace_has(self):
        for area in self.areas:
            self.assertLessEqual(set(area.feeds) - {"*"}, set(self.workspace.folders), area.name)

    def test_the_install_surface_holds_the_installs_files_and_not_their_tests_or_the_cli(self):
        install = self.area("install")
        for path in ("install.sh", "crates/kumi/src/install/migration.rs", "crates/ableton-mcp-server/src/lifecycle_cli.rs",
                     "crates/reaper-kumi/shim/csurf.cpp", "vendor/willington/README.md", ".github/workflows/installer.yml"):
            self.assertTrue(install.holds(path), path)
        for path in ("crates/kumi/src/cli/native/session.rs", "scripts/tests/daw_bridges.py", "scripts/test-isolated.sh",
                     "crates/daw-bridge/tests/it/system.rs", "crates/kumi/src/daw/ableton/willington.rs", "Cargo.lock"):
            self.assertFalse(install.holds(path), path)


if __name__ == "__main__":
    unittest.main()
