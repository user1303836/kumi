"""Native artifact inventory, binding, reproducibility and reference documentation checks."""
import gzip
import hashlib
import importlib.util
import json
import os
import re
import shutil
import tomllib
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("native_release", Path(__file__).parents[1] / "build-native-release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)
# Windows files carry no exec bits, so a bundle built there marks only .exe files executable.
EXEC_BITS = os.name != "nt"

class NativeRelease(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix="kumi-native-release-test-")
        self.root = Path(self.folder.name)
        self.binaries = self.root / "binaries"
        self.binaries.mkdir()
        for name in release.BINARIES:
            (self.binaries / name).write_bytes(b"\x7fELF fixture " + name.encode())
        self.source = {"commit": "a" * 40, "commitTimestamp": "2026-10-01T12:00:00+00:00", "dirty": True}
        self.builder = {"rustc": "rustc fixture", "cargo": "cargo fixture", "platform": "linux", "architecture": "x64",
            "runnerImage": "test", "runnerImageVersion": "test", "cargoLockSha256": "b" * 64, "workflowSha256": "c" * 64}

    def tearDown(self):
        self.folder.cleanup()

    def build(self, out):
        return release.build_release(release.ROOT, self.binaries, out, "x86_64-unknown-linux-gnu", self.source,
                                     self.builder, "test fixture payload (not a compiled runtime)")

    def test_application_and_bridge_versions_match_all_release_inputs(self):
        root = release.ROOT
        read_json = lambda name: json.loads((root / name).read_text())
        cargo_version = lambda name: tomllib.loads((root / name).read_text())["package"]["version"]
        version = cargo_version("crates/kumi/Cargo.toml")
        self.assertEqual(read_json("package.json")["version"], version)
        for name in ("crates/kumi-common/Cargo.toml", "crates/kumi-runtime/Cargo.toml"):
            self.assertEqual(cargo_version(name), version, name)
        declared = re.search(r'KUMI_VERSION[^=]*=\s*"([^"\n]+)"', (root / "crates/kumi-runtime/src/version.rs").read_text())
        self.assertIsNotNone(declared)
        self.assertEqual(declared[1], version)
        cargo_lock = tomllib.loads((root / "Cargo.lock").read_text())["package"]
        for package in cargo_lock:
            if package["name"] in ("kumi", "kumi-runtime", "kumi-common"):
                self.assertEqual(package["version"], version, package["name"])
        bridge = cargo_version("crates/ableton-mcp-server/Cargo.toml")
        self.assertEqual(next(p["version"] for p in cargo_lock if p["name"] == "ableton-mcp-server"), bridge)

    def helper_source(self):
        source = self.root / "crates/kumi-runtime/src/hands/KumiHands.swift"
        source.parent.mkdir(parents=True)
        source.write_text("source fixture")
        return "kumi-hands-" + release.digest(source)[:12]

    def test_mac_release_requires_current_source_helper_and_preserves_legacy_path(self):
        name = self.helper_source()
        bundle = self.root / "bundle"
        with self.assertRaisesRegex(ValueError, "run python3 scripts/build-hands.py first"):
            release.stage_hands(self.root, bundle, "aarch64-apple-darwin")
        helper = self.root / "target/hands" / name
        helper.parent.mkdir(parents=True)
        helper.write_bytes(b"signed universal fixture")
        release.stage_hands(self.root, bundle, "aarch64-apple-darwin")
        destination = bundle / "packages/runtime/hands" / name
        self.assertEqual(destination.read_bytes(), helper.read_bytes())
        if os.name != "nt":
            self.assertEqual(destination.stat().st_mode & 0o777, 0o755)
        self.assertFalse((bundle / "hands").exists())

    def test_mac_release_rejects_stale_source_names_and_linked_helpers(self):
        name = self.helper_source()
        hands = self.root / "target/hands"
        hands.mkdir(parents=True)
        stale = hands / "kumi-hands-000000000000"
        stale.write_bytes(b"old helper")
        with self.assertRaisesRegex(ValueError, "current source"):
            release.stage_hands(self.root, self.root / "bundle", "x86_64-apple-darwin")
        stale.unlink()
        target = self.root / "linked-target"
        target.write_bytes(b"linked helper")
        try:
            (hands / name).symlink_to(target)
        except OSError:
            return  # Windows runners may not grant symlink creation.
        with self.assertRaisesRegex(ValueError, "link or special file"):
            release.stage_hands(self.root, self.root / "bundle", "aarch64-apple-darwin")

    def test_non_mac_and_bridge_only_releases_do_not_require_hands(self):
        self.helper_source()
        for target in ("x86_64-unknown-linux-gnu", "aarch64-pc-windows-msvc"):
            release.stage_hands(self.root, self.root / "bundle", target)
        self.assertFalse((self.root / "bundle").exists())
        with patch.object(release, "stage_hands", side_effect=AssertionError("bridge-only called hands staging")):
            result = release.build_release(release.ROOT, self.binaries, self.root / "bridge", "aarch64-apple-darwin",
                                           self.source, self.builder, "fixture", bridge_only=True)
        self.assertEqual(result["manifest"]["build"]["target"], "aarch64-apple-darwin")

    def test_native_bundle_has_exact_manifest_bound_bridge_and_no_node_runtime(self):
        out = self.root / "release"
        result = self.build(out)
        archive = out / result["bundle"]
        self.assertEqual(release.digest(archive), result["sha256"])
        self.assertEqual(result["runtime"], "rust-native")
        self.assertNotIn("node", result)
        with tarfile.open(archive) as tar:
            self.assertTrue(all(member.isfile() for member in tar.getmembers()))
            names = tar.getnames()
            self.assertTrue(all(binary in names for binary in release.BINARIES))
            self.assertIn("apps/kumi/bin/kumi.mjs", names)
            helper_name = "kumi-hands-" + release.digest(release.ROOT / "crates/kumi-runtime/src/hands/KumiHands.swift")[:12]
            source_helper = release.ROOT / "target/hands" / helper_name
            if source_helper.is_file():
                helper_path = "packages/runtime/hands/" + helper_name
                self.assertEqual(tar.extractfile(helper_path).read(), source_helper.read_bytes())
                if EXEC_BITS:
                    self.assertEqual(tar.getmember(helper_path).mode, 0o755)
                self.assertNotIn("hands/" + helper_name, names)
            self.assertEqual(json.load(tar.extractfile("apps/mcp-server/package.json"))["version"], result["bridge"])
            self.assertFalse(any("node_modules" in name or name.startswith("node/") for name in names))
            prepared = json.load(tar.extractfile("bridge/prepared.json"))
            artifact = tar.extractfile("bridge/" + prepared["artifact"]).read()
            self.assertEqual(hashlib.sha256(artifact).hexdigest(), prepared["sha256"])
            manifest = json.load(tar.extractfile("bridge/package/release-manifest.json"))
            self.assertEqual(manifest["schema"], "ableton-mcp-native-release/v1")
            self.assertEqual(manifest["source"], self.source)
            self.assertEqual(manifest["build"]["builder"], self.builder)
            self.assertEqual(manifest["build"]["runtime"], "rust-native")
            self.assertEqual(manifest["roles"]["ableton-mcp-server"], "native-runtime")
            self.assertEqual(manifest["roles"]["ableton-mcp-analysis-worker"], "native-runtime")
            package = json.load(tar.extractfile("bridge/package/package.json"))
            self.assertEqual(package["bin"], {binary: binary for binary in release.BRIDGE_BINARIES})
            self.assertNotIn("compiled-runtime", manifest["roles"].values())
            self.assertEqual(manifest["files"]["LICENSE.md"], release.MIT_SHA256)
            self.assertEqual(set(manifest["files"]), set(manifest["roles"]))
            self.assertNotIn("release-manifest.json", manifest["files"])
            self.assertGreaterEqual(len(manifest["files"]), 10)
            # The canonical JSON hash of the protocol registry.
            self.assertEqual(manifest["protocol"]["registryHash"], "f626dafa686101c714ca610f4c47689f9297b19c838349e3bd7ef46614de4fa8")
            self.assertEqual(manifest["distribution"], {"channel": "local-native-tarball", "published": False,
                "signed": False, "notarized": False, "integrityIsIdentityProof": False})
            for name, digest in manifest["files"].items():
                self.assertEqual(hashlib.sha256(tar.extractfile("bridge/package/" + name).read()).hexdigest(), digest)
            import io
            with tarfile.open(fileobj=io.BytesIO(artifact)) as bridge:
                expected = {"package/" + name for name in manifest["files"]} | {"package/release-manifest.json"}
                self.assertEqual(set(bridge.getnames()), expected)
                self.assertTrue(all(member.isfile() and not member.pax_headers for member in bridge.getmembers()))
                for binary in release.BRIDGE_BINARIES:
                    if EXEC_BITS:
                        self.assertEqual(bridge.getmember("package/" + binary).mode, 0o755)
                for member in bridge.getmembers():
                    self.assertEqual(bridge.extractfile(member).read(), tar.extractfile("bridge/" + member.name).read())
        self.assertTrue(gzip.decompress(artifact).endswith(b"\0" * 1024))

    def test_archive_is_reproducible_and_failed_rebuild_preserves_previous_bundle(self):
        out = self.root / "release"
        result = self.build(out)
        first = (out / result["bundle"]).read_bytes()
        self.assertEqual(self.build(out), result)
        self.assertEqual((out / result["bundle"]).read_bytes(), first)
        (self.binaries / "kumi-harness").unlink()
        with self.assertRaises(ValueError):
            self.build(out)
        self.assertEqual((out / result["bundle"]).read_bytes(), first)
        self.assertFalse(list(out.glob("kumi-native-stage-*")))

    def test_bridge_only_requires_analysis_worker_and_supports_windows_names(self):
        (self.binaries / "ableton-mcp-analysis-worker").unlink()
        with self.assertRaises(ValueError):
            release.build_release(release.ROOT, self.binaries, self.root / "missing", "x86_64-unknown-linux-gnu",
                                  self.source, self.builder, "fixture", bridge_only=True)
        for name in release.BRIDGE_BINARIES:
            (self.binaries / (name + ".exe")).write_bytes(b"MZ fixture " + name.encode())
        out = self.root / "windows"
        result = release.build_release(release.ROOT, self.binaries, out, "x86_64-pc-windows-msvc",
                                       self.source, self.builder, "fixture", bridge_only=True)
        with tarfile.open(out / result["artifact"]) as archive:
            package = json.load(archive.extractfile("package/package.json"))
            for name in release.BRIDGE_BINARIES:
                self.assertEqual(package["bin"][name], name + ".exe")
                self.assertEqual(result["manifest"]["roles"][name + ".exe"], "native-runtime")
                self.assertEqual(archive.getmember("package/" + name + ".exe").mode, 0o755)

    def test_manifest_rejects_unknown_roles_and_linked_payloads(self):
        with self.assertRaises(ValueError):
            release.role("local-state/auth.json")
        with self.assertRaises(ValueError):
            release.role("dist/src/cli.js")
        source = self.root / "payload"
        source.mkdir()
        (source / "file").write_text("content")
        try:
            (source / "link").symlink_to(source / "file")
        except OSError:
            self.skipTest("this Windows user cannot create symlinks")
        with self.assertRaises(ValueError):
            release.inventory(source)
        with self.assertRaises(ValueError):
            release.copy(source / "link", self.root / "copied")

    def vendor_willington(self, files=None, **manifest):
        """vendor/willington in the fixture root, as Willington's sync writes it (replacing any earlier one)."""
        folder = self.root / release.WILLINGTON
        shutil.rmtree(folder, ignore_errors=True)
        files = files or {"LICENSE": b"Willington license fixture\n", "WillingtonRuntime/__init__.py": b"def resolve(*args): pass\n",
            "WillingtonRuntime/matrix.json": b"{}\n", "WillingtonDeviceTools/api.py": b"def install(): pass\n",
            "WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64/willington_devices.pyd": b"MZ fixture\0",
            "WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64/build.json": b"{}\n"}
        for name, content in files.items():
            (folder / name).parent.mkdir(parents=True, exist_ok=True)
            (folder / name).write_bytes(content)
        listed = {name: hashlib.sha256(content).hexdigest() for name, content in files.items()}
        (folder / "release.json").write_text(json.dumps({"schema": release.WILLINGTON_SCHEMA, "version": "0.4.0",
            "commit": "d" * 40, "files": listed, **manifest}))
        return folder, files

    def test_vendored_willington_is_staged_inside_the_bridge(self):
        remote = self.root / "package/remote-script/AbletonMcpBridge"
        release.stage_willington(self.root, remote, "x86_64-pc-windows-msvc")
        self.assertFalse(remote.exists())
        folder, files = self.vendor_willington()
        release.stage_willington(self.root, remote, "x86_64-pc-windows-msvc")
        staged = release.inventory(remote / "willington")
        self.assertEqual(set(staged), {*files, "release.json"})
        self.assertEqual(staged, release.inventory(folder))
        for name in staged:
            self.assertEqual(release.role("remote-script/AbletonMcpBridge/willington/" + name), "ableton-remote-script")

    def test_each_bundle_carries_willingtons_native_libraries_for_its_own_platform_only(self):
        dylib = "WillingtonDeviceTools/build/live-12.4.15b5-arm64/libwillington_devices.dylib"
        _, files = self.vendor_willington()
        pyd = next(name for name in files if name.endswith(".pyd"))
        folder, files = self.vendor_willington({**files, dylib: b"\xcf\xfa\xed\xfe fixture"})
        for target, native, other in [("x86_64-pc-windows-msvc", pyd, dylib), ("aarch64-apple-darwin", dylib, pyd),
                                      ("x86_64-apple-darwin", dylib, pyd)]:
            with self.subTest(target=target):
                remote = self.root / target / "AbletonMcpBridge"
                release.stage_willington(self.root, remote, target)
                staged = release.inventory(remote / "willington")
                self.assertIn(native, staged)
                self.assertNotIn(other, staged)
                # release.json stays Willington's own, naming every platform's files.
                self.assertEqual(staged["release.json"], release.digest(folder / "release.json"))
        # Live doesn't run on Linux: nothing to carry there.
        remote = self.root / "linux" / "AbletonMcpBridge"
        release.stage_willington(self.root, remote, "x86_64-unknown-linux-gnu")
        self.assertFalse(remote.exists())
        # The size cap is for what one platform's bundle carries, not every platform's libraries together.
        common = sum(len(content) for name, content in files.items() if not name.endswith((".pyd", ".dylib")))
        largest = max(len(files[pyd]), len(files[dylib]))
        with patch.object(release, "WILLINGTON_MAX_BYTES", common + largest):
            release.willington_files(folder)
        with patch.object(release, "WILLINGTON_MAX_BYTES", common + largest - 1):
            with self.assertRaisesRegex(ValueError, "larger than"):
                release.willington_files(folder)

    def test_vendored_willington_names_must_check_out_on_every_platform(self):
        # A backslash or a colon is an ordinary character on macOS and Linux; Windows can't check either out,
        # nor a device name or a name ending in a dot. (Names, not files: this system may not hold them either.)
        for name in ("WillingtonRuntime/x.py\\..\\y.py", "WillingtonRuntime/aux.json", "WillingtonRuntime/CON",
                     "WillingtonRuntime/lpt1.py", "WillingtonRuntime/x.py.", "WillingtonRuntime/..", "WillingtonRuntime//x.py",
                     "WillingtonRuntime/x:y.py", "WillingtonRuntime/ü.py", "WillingtonRuntime/a b.py"):
            self.assertFalse(release.portable(name), name)
        for name in ("WillingtonRuntime/console.py", "WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64/build.json"):
            self.assertTrue(release.portable(name), name)
        _, files = self.vendor_willington()
        folder, _ = self.vendor_willington({**files, "WillingtonRuntime/a b.py": b"fixture"})
        with self.assertRaisesRegex(ValueError, "some systems can't hold: WillingtonRuntime/a b.py"):
            release.willington_files(folder)
        # Two names one letter's case apart are one file on macOS and Windows.
        folder, _ = self.vendor_willington({**files, "WillingtonRuntime/Matrix.json": b"{}\n"})
        with self.assertRaisesRegex(ValueError, "differ only in case"):
            release.willington_files(folder)
        folder, _ = self.vendor_willington({**files, "WillingtonRuntime/console_check.py": b"fixture"})
        release.willington_files(folder)

    def test_vendored_willington_refuses_anything_but_its_listed_runtime_files(self):
        folder, files = self.vendor_willington()
        (folder / "WillingtonDeviceTools/native_windows.cpp").write_text("// stays in Willington's repository")
        with self.assertRaisesRegex(ValueError, "differs from its release.json: WillingtonDeviceTools/native_windows.cpp"):
            release.willington_files(folder)
        # Listed or not, a source, a header, a debug file or a bytecode cache never ships.
        for name in ("WillingtonDeviceTools/native_windows.cpp", "WillingtonRuntime/windows_image.hpp",
                     "WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64/willington_devices.pdb",
                     "WillingtonRuntime/__pycache__/__init__.cpython-311.pyc", "profiles/live-12.4.15b5-windows-x86_64.json"):
            with self.subTest(name=name):
                self.vendor_willington({**files, name: b"fixture"})
                with self.assertRaisesRegex(ValueError, "isn't one of Willington's runtime files"):
                    release.willington_files(folder)
        self.vendor_willington()
        (folder / "WillingtonRuntime/matrix.json").write_bytes(b'{"changed": true}\n')
        with self.assertRaisesRegex(ValueError, "matrix.json doesn't match its SHA-256"):
            release.willington_files(folder)
        for left_out, refused in [("LICENSE", "license notice"), ("WillingtonRuntime/matrix.json", "missing WillingtonRuntime/matrix.json")]:
            with self.subTest(left_out=left_out):
                self.vendor_willington({name: content for name, content in files.items() if name != left_out})
                with self.assertRaisesRegex(ValueError, refused):
                    release.willington_files(folder)
        for manifest, refused in [({"schema": "other/v1"}, "manifest"), ({"extra": True}, "manifest"),
                                  ({"commit": "main"}, "invalid commit"), ({"version": "../1"}, "invalid version")]:
            with self.subTest(manifest=manifest):
                self.vendor_willington(None, **manifest)
                with self.assertRaisesRegex(ValueError, refused):
                    release.willington_files(folder)

    def test_the_repository_vendors_only_listed_willington_runtime_files(self):
        # Willington's sync pull requests change vendor/willington: CI checks them here, before a release ships them.
        release.willington_files(release.ROOT / release.WILLINGTON)

    def test_every_packaged_document_rewrites_its_links(self):
        revision = "a" * 40
        # A relative link to a missing file raises: every packaged document's links resolve, to another
        # packaged document or to this revision on GitHub.
        for source, _ in release.DOCUMENTS:
            release.transform_document((release.ROOT / source).read_text(encoding="utf-8"), release.ROOT, source, revision)
        self.assertEqual(release.document_target(release.ROOT, "docs/en/test.md", "../../crates/ableton-mcp-server/README.md", revision, "href"), "README.md")
        for target in ("../../../../outside", "/absolute", "C:/user", "a\\b", "x\0y"):
            with self.assertRaises(ValueError):
                release.document_target(release.ROOT, "docs/en/test.md", target, revision, "href")

if __name__ == "__main__":
    unittest.main()
