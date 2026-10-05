"""Native artifact inventory, binding, reproducibility and reference documentation checks."""
import gzip
import hashlib
import importlib.util
import json
import os
import re
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
            self.assertEqual(manifest["protocol"]["registryHash"], "ec05dd401ec098adb77da1c185aff1857be2bd87859afe9dda4bfeb14e04aa57")
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
