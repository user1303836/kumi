"""setup.py (C8): status, plan, approval and install, as the plugin's tools, hook, CLI and backend use them.

No network, no Hermes, no Kumi: the release is a local .tar.xz served by a fake opener, and the runtime's setup_daw is
a function that answers the way W1's does (W1-REGISTRY.md, comments).
"""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("kumi_setup", Path(__file__).resolve().parents[1] / "setup.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)

TARGET, VERSION = "aarch64-apple-darwin", "1.24.67"
URL = f"https://github.com/user1303836/kumi/releases/download/v{VERSION}/kumi-{TARGET}.tar.xz"


def tar_xz(files: dict[str, bytes], links: dict[str, str] | None = None) -> bytes:
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:xz") as archive:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size, info.mode = len(data), 0o755
            archive.addfile(info, io.BytesIO(data))
        for name, target in (links or {}).items():
            info = tarfile.TarInfo(name)
            info.type, info.linkname = tarfile.SYMTYPE, target
            archive.addfile(info)
    return out.getvalue()


BUNDLE = tar_xz({"kumi": b"#!/bin/sh\necho kumi\n", "daw-mcp-server": b"host", "README.md": b"readme"})


class Served:
    """A fake urlopen: serves `data` for `URL` and records what was asked for."""

    def __init__(self, data: bytes) -> None:
        self.data, self.asked = data, []

    def __call__(self, url: str):
        self.asked.append(url)
        return io.BytesIO(self.data)


class Runtime:
    """The runtime's setup_daw as W1 answers it: a dry run names the paths; a real install does exactly that."""

    def __init__(self, state: str = "missing", owner: str | None = None) -> None:
        self.state, self.owner, self.calls = state, owner, []

    def __call__(self, args: dict) -> dict:
        self.calls.append(args)
        reply = {"daw": args["daw"], "component": "Live Remote Script", "version": "1.0.105",
                 "paths": ["/Users/x/Music/Ableton/User Library/Remote Scripts/AbletonMcpBridge"],
                 "state": self.state, "owner": {"kumi_home": self.owner} if self.owner else None,
                 "after": "choose_control_surface",
                 "effects": ["Copy the AbletonMcpBridge Remote Script into /Users/x/Music/Ableton/User Library/Remote Scripts"]}
        if not args["dry_run"]:
            reply.update(state="same", installed_at=1730764800000)
        return reply


class SetupTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.plugin, self.data = root / "plugin", root / "data"
        self.plugin.mkdir()
        self.pin(BUNDLE)
        # A standalone Kumi on the test machine must not leak into plans.
        patcher = mock.patch.dict(os.environ, {"KUMI_HOME": str(root / "no-standalone-kumi")})
        patcher.start()
        self.addCleanup(patcher.stop)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def pin(self, bundle: bytes, **changes) -> None:
        record = {"url": URL, "sha256": hashlib.sha256(bundle).hexdigest(), "size": len(bundle),
                  "unpacked_bytes": 1234, "bridge": "1.0.105"}
        record.update(changes)
        (self.plugin / "kumi_release.json").write_text(json.dumps(
            {"schema": 1, "kumi": VERSION, "repository": "user1303836/kumi", "tag": f"v{VERSION}", "targets": {TARGET: record}}))

    def plan(self, **kw) -> dict:
        return setup.plan("ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET, **kw)

    def install(self, served: Served, **kw) -> dict:
        return setup.install("ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET, opener=served, **kw)

    def status(self) -> dict:
        return setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET)

    def install_runtime(self) -> None:
        self.plan()
        self.assertTrue(self.install(Served(BUNDLE))["ok"])

    def adopt(self, runtime) -> dict:
        return setup.adopt("ableton", setup_daw=runtime, data_dir=self.data, plugin_dir=self.plugin, target=TARGET)

    def test_host_target_names_kumis_bundles_and_nothing_else(self) -> None:
        self.assertEqual(setup.host_target("Darwin", "arm64"), "aarch64-apple-darwin")
        self.assertEqual(setup.host_target("Windows", "AMD64"), "x86_64-pc-windows-msvc")
        self.assertEqual(setup.host_target("Linux", "aarch64"), "aarch64-unknown-linux-gnu")
        self.assertIsNone(setup.host_target("FreeBSD", "amd64"))
        self.assertIsNone(setup.host_target("Linux", "riscv64"))

    def test_status_before_setup_says_what_is_needed_without_writing_anything(self) -> None:
        state = self.status()
        self.assertEqual((state["state"], state["next_step"], state["pinned_version"]), ("needs_runtime", "install_runtime", VERSION))
        self.assertEqual(state["download_bytes"], len(BUNDLE))
        self.assertFalse(self.data.exists(), "status() must not create the data folder")
        (self.plugin / "kumi_release.json").unlink()
        self.assertEqual(self.status()["state"], "no_pin")
        self.assertEqual(setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target="riscv64-x")["state"], "no_pin")

    def test_status_on_a_machine_kumi_has_no_build_for(self) -> None:
        self.pin(BUNDLE)
        state = setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target="x86_64-unknown-linux-gnu")
        self.assertEqual(state["state"], "unsupported_platform")

    def test_the_runtime_plan_names_the_exact_download_and_folder_and_needs_approval(self) -> None:
        plan = self.plan()
        self.assertEqual(plan["step"], "runtime")
        self.assertEqual(plan["download"]["url"], URL)
        self.assertIn(URL, plan["effects"][0])
        self.assertEqual(plan["paths"], [str(self.data / "runtime" / VERSION)])
        text = setup.approval("ableton", data_dir=self.data)
        self.assertIn(URL, text)
        self.assertIsNone(setup.approval("reaper", data_dir=self.data), "a plan for Live doesn't approve REAPER")
        later = plan["made_at"] + setup.PLAN_TTL_MS + 1
        self.assertIsNone(setup.approval("ableton", data_dir=self.data, now=later), "a stale plan approves nothing")

    def test_installing_the_runtime_verifies_extracts_and_records_it_for_runtime_py(self) -> None:
        self.plan()
        served = Served(BUNDLE)
        result = self.install(served)
        self.assertTrue(result["ok"], result)
        self.assertEqual(served.asked, [URL])
        current = json.loads((self.data / "runtime" / "current.json").read_text())
        self.assertEqual({k: current[k] for k in ("schema", "version", "target", "kumi", "sha256")},
                         {"schema": 1, "version": VERSION, "target": TARGET, "kumi": f"runtime/{VERSION}/kumi",
                          "sha256": hashlib.sha256(BUNDLE).hexdigest()})
        self.assertTrue(os.access(self.data / current["kumi"], os.X_OK))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")
        self.assertEqual(self.status()["state"], "needs_daw_component")
        self.assertEqual(sorted(p.name for p in (self.data / "runtime").iterdir()), ["1.24.67", "current.json"],
                         "no download, staging or old folders are left behind")

    def test_an_install_needs_a_fresh_plan_and_one_plan_installs_once(self) -> None:
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "no_plan")
        self.plan()
        self.assertTrue(self.install(Served(BUNDLE))["ok"])
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "no_plan")

    def test_a_plan_that_changed_since_approval_installs_nothing(self) -> None:
        self.plan()
        other = tar_xz({"kumi": b"another build"})
        self.pin(other)
        served = Served(other)
        self.assertEqual(self.install(served)["error"]["code"], "plan_changed")
        self.assertEqual(served.asked, [])

    def test_a_download_that_differs_from_the_pin_installs_nothing(self) -> None:
        self.plan()
        result = self.install(Served(tar_xz({"kumi": b"tampered"})))
        self.assertEqual(result["error"]["code"], "checksum_mismatch")
        self.assertFalse((self.data / "runtime" / "current.json").exists())
        self.assertEqual([p.name for p in (self.data / "runtime").iterdir()], [])

    def test_a_data_folder_setup_cannot_write_in_is_reported_not_raised(self) -> None:
        self.data.mkdir()
        (self.data / "setup").write_text("a file where setup keeps its plan")
        with self.assertRaises(setup.SetupError) as raised:
            self.plan()
        self.assertEqual(raised.exception.code, "io_failed")
        (self.data / "setup").unlink()
        self.plan()
        (self.data / "runtime").write_text("a file where the runtime goes")
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "io_failed")

    def test_a_bundle_setup_would_not_unpack_installs_nothing(self) -> None:
        for problem, bundle in {"a symlink": tar_xz({"kumi": b"k"}, links={"escape": "/etc"}),
                                "a climbing path": tar_xz({"kumi": b"k", "../outside": b"x"}),
                                "no kumi": tar_xz({"README.md": b"r"}),
                                "only a lookalike of kumi": tar_xz({".kumi": b"k", "bin/kumi": b"k"})}.items():
            with self.subTest(problem):
                self.pin(bundle)
                self.plan()
                self.assertEqual(self.install(Served(bundle))["error"]["code"], "bad_bundle")
                self.assertFalse((self.data / "runtime" / "current.json").exists())
                self.assertFalse((self.data.parent / "outside").exists())

    def test_a_pin_pointing_anywhere_but_the_pinned_release_is_refused_before_downloading(self) -> None:
        for bad in ({"url": "https://github.com/user1303836/kumi/releases/latest/download/kumi-aarch64-apple-darwin.tar.xz"},
                    {"url": URL.replace("github.com", "example.com")},
                    {"sha256": "not-a-digest"}, {"size": 0}):
            with self.subTest(bad):
                self.pin(BUNDLE, **bad)
                with self.assertRaises(setup.SetupError) as raised:
                    self.plan()
                self.assertEqual(raised.exception.code, "bad_pin")

    def test_the_daw_step_is_planned_from_the_runtimes_dry_run_and_does_exactly_that(self) -> None:
        self.install_runtime()
        runtime = Runtime()
        plan = self.plan(setup_daw=runtime)
        self.assertEqual(plan["step"], "daw_component")
        self.assertEqual(runtime.calls, [{"daw": "ableton", "dry_run": True}])
        self.assertIn("Remote Scripts", setup.approval("ableton", data_dir=self.data))
        result = self.install(Served(b""), setup_daw=runtime)
        self.assertTrue(result["ok"], result)
        self.assertEqual(runtime.calls[-1], {"daw": "ableton", "dry_run": False})
        state = self.status()
        self.assertEqual((state["state"], state["next_step"]), ("ready", "choose_control_surface"))
        self.assertEqual(state["daw_component"]["version"], "1.0.105")
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none")

    def test_a_component_another_kumi_owns_is_never_overwritten(self) -> None:
        self.install_runtime()
        runtime = Runtime("other_owner")
        self.assertEqual(self.plan(setup_daw=runtime)["step"], "owned_elsewhere")
        self.assertIsNone(setup.approval("ableton", data_dir=self.data))
        self.assertEqual(self.install(Served(b""), setup_daw=runtime)["error"]["code"], "owned_elsewhere")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, runtime.calls)

    def test_a_runtime_that_refuses_or_fails_is_a_setup_error_with_its_code(self) -> None:
        self.install_runtime()

        class Refusal(Exception):  # what the runtime client raises for an isError answer
            code, reason = "other_daw", "Kumi's runtime in this profile works in REAPER"

        def refuses(args: dict) -> dict:
            raise Refusal()

        with self.assertRaises(setup.SetupError) as raised:
            self.plan(setup_daw=refuses)
        self.assertEqual(raised.exception.code, "other_daw")
        self.assertIn("works in REAPER", raised.exception.message)
        self.plan(setup_daw=Runtime())

        def dies_installing(args: dict) -> dict:
            if args["dry_run"]:
                return Runtime()(args)
            raise RuntimeError("Kumi's runtime stopped")

        result = self.install(Served(b""), setup_daw=dies_installing)
        self.assertEqual(result["error"]["code"], "daw_step_failed")
        self.assertIn("runtime stopped", result["error"]["message"])
        self.assertEqual(self.status()["daw_component"]["state"], "not_installed")

    def test_a_component_this_kumi_installed_before_is_recorded_when_planning(self) -> None:
        self.install_runtime()
        self.assertEqual(self.status()["state"], "needs_daw_component")
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none")
        state = self.status()
        self.assertEqual((state["state"], state["daw_component"]["state"]), ("ready", "installed"))

    def test_adopting_the_installation_that_owns_the_component_records_only_its_folder(self) -> None:
        self.install_runtime()
        standalone = self.data.parent / "standalone-kumi"
        standalone.mkdir()
        runtime = Runtime("other_owner", owner=str(standalone))
        self.assertEqual(self.adopt(runtime)["error"]["code"], "no_plan")
        self.plan(setup_daw=runtime)
        self.assertIsNone(setup.approval("ableton", data_dir=self.data), "install never approves an adoption")
        text = setup.approval("ableton", action="adopt", data_dir=self.data)
        self.assertIn(str(standalone), text)
        self.assertIn("never its sign-in", text)
        result = self.adopt(runtime)
        self.assertTrue(result["ok"], result)
        current = json.loads((self.data / "runtime" / "current.json").read_text())
        self.assertEqual(current["kumi_home"], str(standalone))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")
        self.assertEqual(self.status()["daw_component"]["state"], "installed")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, runtime.calls, "adoption installs nothing")

    def test_an_adoption_whose_owner_changed_or_vanished_is_refused(self) -> None:
        self.install_runtime()
        first, second = self.data.parent / "kumi-a", self.data.parent / "kumi-b"
        first.mkdir()
        second.mkdir()
        self.plan(setup_daw=Runtime("other_owner", owner=str(first)))
        self.assertEqual(self.adopt(Runtime("other_owner", owner=str(second)))["error"]["code"], "plan_changed")
        self.plan(setup_daw=Runtime("other_owner", owner=str(first)))
        first.rmdir()
        self.assertEqual(self.adopt(Runtime("other_owner", owner=str(first)))["error"]["code"], "not_found")
        self.assertNotIn("kumi_home", json.loads((self.data / "runtime" / "current.json").read_text()))

    def test_a_record_that_points_outside_the_data_folder_is_damaged(self) -> None:
        self.install_runtime()
        current = self.data / "runtime" / "current.json"
        record = json.loads(current.read_text())
        current.write_text(json.dumps({**record, "kumi": "../../bin/sh"}))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "damaged")
        self.assertEqual(self.status()["next_step"], "install_runtime")
        current.write_text(json.dumps(record))
        binary = self.data / record["kumi"]
        binary.unlink()
        binary.symlink_to("/bin/sh")
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "damaged",
                         "a binary that is a link out of the data folder isn't trusted")

    def test_the_cli_asks_before_each_step_and_refuses_without_a_terminal(self) -> None:
        class Tty(io.StringIO):
            def isatty(self) -> bool:
                return True

        def cli(answers: str, stdin_is_tty: bool = True) -> tuple[int, str]:
            out = Tty()
            stdin = Tty(answers) if stdin_is_tty else io.StringIO(answers)
            code = setup.cli_main("ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET,
                                  opener=Served(BUNDLE), stdin=stdin, stdout=out)
            return code, out.getvalue()

        code, said = cli("y\n", stdin_is_tty=False)
        self.assertEqual(code, 1)
        self.assertIn("needs a terminal", said)
        self.assertFalse(self.data.exists(), "without a terminal nothing is planned or installed")
        code, said = cli("n\n")
        self.assertEqual(code, 1)
        self.assertIn(URL, said)
        self.assertFalse((self.data / "runtime" / "current.json").exists())
        code, said = cli("y\n")
        self.assertEqual(code, 0, said)
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")


if __name__ == "__main__":
    unittest.main()
