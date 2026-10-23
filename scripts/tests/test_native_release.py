"""Native artifact inventory, binding, reproducibility and reference documentation checks."""
import gzip
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import stat
import struct
import sys
import tomllib
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

VENDOR_SPEC = importlib.util.spec_from_file_location("vendor_willington", Path(__file__).parents[1] / "vendor-willington.py")
vendor = importlib.util.module_from_spec(VENDOR_SPEC)
VENDOR_SPEC.loader.exec_module(vendor)
# The release script as the update script loaded it: one module, so a patch on it reaches both.
release = vendor.release
# Windows files carry no exec bits, so a bundle built there marks only .exe files executable.
EXEC_BITS = os.name != "nt"

# Libraries as each system's linker lays them out, cut down to what the release script reads: their exported names
# and, by format, what REAPER's loader on that system cares about.

def elf_library(machine=62, exports=("ReaperPluginEntry",), glibc=("2.2.5", "2.34")):
    strings = bytearray(b"\0")
    def name(text):
        strings.extend(text.encode() + b"\0")
        return len(strings) - len(text) - 1
    symbols = [struct.pack("<IBBHQQ", 0, 0, 0, 0, 0, 0)]
    symbols += [struct.pack("<IBBHQQ", name(export), 0x12, 0, 7, 0x1000, 8) for export in exports]
    symbols.append(struct.pack("<IBBHQQ", name("memcpy"), 0x12, 0, 0, 0, 0))  # imported: not defined here
    symbols.append(struct.pack("<IBBHQQ", name("hidden_helper"), 0x12, 2, 7, 0x1100, 8))  # hidden: not exported
    libc = name("libc.so.6")
    aux = b"".join(struct.pack("<IHHII", 0, 0, i + 2, name(f"GLIBC_{version}"), 16 if i + 1 < len(glibc) else 0)
                   for i, version in enumerate(glibc))
    needed = struct.pack("<HHIII", 1, len(glibc), libc, 16, 0) + aux
    names = b"\0.dynstr\0.dynsym\0.gnu.version_r\0.shstrtab\0"
    blobs = [bytes(strings), b"".join(symbols), needed, names]
    offsets, at = [], 64
    for blob in blobs:
        offsets.append(at)
        at += len(blob)
    table = (at + 7) & ~7
    header = b"\x7fELF\x02\x01\x01" + b"\0" * 9 + struct.pack("<HHIQQQIHHHHHH", 3, machine, 1, 0, 0, table, 0, 64, 0, 0, 64, 5, 4)
    sections = [b"\0" * 64,
                struct.pack("<IIQQQQIIQQ", 1, 3, 2, 0, offsets[0], len(blobs[0]), 0, 0, 1, 0),
                struct.pack("<IIQQQQIIQQ", 9, 11, 2, 0, offsets[1], len(blobs[1]), 1, 1, 8, 24),
                struct.pack("<IIQQQQIIQQ", 17, 0x6FFFFFFE, 2, 0, offsets[2], len(blobs[2]), 1, 1, 8, 0),
                struct.pack("<IIQQQQIIQQ", 32, 3, 0, 0, offsets[3], len(blobs[3]), 0, 0, 1, 0)]
    data = header + b"".join(blobs)
    return data + b"\0" * (table - len(data)) + b"".join(sections)

def macho_library(cpu=0x0100000C, exports=("ReaperPluginEntry",), install_name="@rpath/reaper_kumi.dylib", signed=True):
    edges = [b"_" + export.encode() + b"\0" for export in exports]
    root = 2 + sum(len(edge) + 1 for edge in edges)
    trie = bytes([0, len(edges)]) + b"".join(edge + bytes([root + 4 * i]) for i, edge in enumerate(edges))
    trie += b"".join(bytes([2, 0, 0x10, 0]) for _ in edges)  # each a terminal (flags 0, address 0x10), no children
    name = install_name.encode() + b"\0"
    name += b"\0" * (-len(name) % 8)
    commands = [struct.pack("<IIIIII", 0xD, 24 + len(name), 24, 0, 0, 0) + name]
    trie_at = 32 + sum(map(len, commands)) + 16 + (16 if signed else 0)
    commands.append(struct.pack("<IIII", 0x80000033, 16, trie_at, len(trie)))
    if signed:
        commands.append(struct.pack("<IIII", 0x1D, 16, trie_at + len(trie), 16))
    header = struct.pack("<IiIIIIII", 0xFEEDFACF, cpu, 0, 6, len(commands), sum(map(len, commands)), 0, 0)
    return header + b"".join(commands) + trie + (b"\0" * 16 if signed else b"")

def universal(*slices):
    data = bytearray(struct.pack(">II", 0xCAFEBABE, len(slices)))
    offset = 4096
    entries, bodies = [], []
    for piece in slices:
        entries.append(struct.pack(">iIIII", struct.unpack_from("<i", piece, 4)[0], 0, offset, len(piece), 12))
        bodies.append((offset, piece))
        offset += (len(piece) + 4095) // 4096 * 4096
    data += b"".join(entries)
    for at, piece in bodies:
        data += b"\0" * (at - len(data)) + piece
    return bytes(data)

def pe_library(machine=0x8664, exports=("ReaperPluginEntry",), imports=("KERNEL32.dll",), delayed=()):
    base, raw = 0x1000, 0x200
    blob = bytearray()
    def put(data):
        blob.extend(data)
        return base + len(blob) - len(data)
    names = [put(name.encode() + b"\0") for name in exports]
    table = put(b"".join(struct.pack("<I", rva) for rva in names))
    export_dir = put(struct.pack("<IIHHIIIIIII", 0, 0, 0, 0, 0, 1, len(exports), len(exports), 0, table, 0))
    dlls = [put(name.encode() + b"\0") for name in imports]
    import_dir = put(b"".join(struct.pack("<IIIII", 1, 0, 0, rva, 1) for rva in dlls) + b"\0" * 20)
    late = [put(name.encode() + b"\0") for name in delayed]
    delay_dir = put(b"".join(struct.pack("<IIIIIIII", 1, rva, 0, 0, 0, 0, 0, 0) for rva in late) + b"\0" * 32)
    optional = bytearray(240)
    struct.pack_into("<H", optional, 0, 0x20B)
    struct.pack_into("<I", optional, 108, 16)
    struct.pack_into("<II", optional, 112, export_dir, 40)
    struct.pack_into("<II", optional, 120, import_dir, 20 * (len(imports) + 1))
    if delayed:
        struct.pack_into("<II", optional, 112 + 8 * 13, delay_dir, 32 * (len(delayed) + 1))
    section = struct.pack("<8sIIIIIIHHI", b".rdata", len(blob), base, len(blob), raw, 0, 0, 0, 0, 0x40000040)
    head = bytearray(b"MZ" + b"\0" * 0x3E)
    struct.pack_into("<I", head, 0x3C, 0x40)
    head += b"PE\0\0" + struct.pack("<HHIIIHH", machine, 1, 0, 0, 0, len(optional), 0x2022) + optional + section
    return bytes(head) + b"\0" * (raw - len(head)) + bytes(blob)

class NativeRelease(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix="kumi-native-release-test-")
        self.root = Path(self.folder.name)
        self.binaries = self.root / "binaries"
        self.binaries.mkdir()
        for name in release.BINARIES:
            (self.binaries / name).write_bytes(b"\x7fELF fixture " + name.encode())
        # REAPER's extension as the Linux x64 bundle's job finishes it.
        self.extension = self.root / "reaper_kumi.so"
        self.extension.write_bytes(elf_library())
        self.source = {"commit": "a" * 40, "commitTimestamp": "2026-10-01T12:00:00+00:00", "dirty": True}
        self.builder = {"rustc": "rustc fixture", "cargo": "cargo fixture", "platform": "linux", "architecture": "x64",
            "runnerImage": "test", "runnerImageVersion": "test", "cargoLockSha256": "b" * 64, "workflowSha256": "c" * 64}

    def tearDown(self):
        self.folder.cleanup()

    def build(self, out):
        return release.build_release(release.ROOT, self.binaries, out, "x86_64-unknown-linux-gnu", self.source,
                                     self.builder, "test fixture payload (not a compiled runtime)", reaper_extension=self.extension)

    def test_application_and_bridge_versions_match_all_release_inputs(self):
        root = release.ROOT
        read_json = lambda name: json.loads((root / name).read_text())
        cargo_version = lambda name: tomllib.loads((root / name).read_text())["package"]["version"]
        version = cargo_version("crates/kumi/Cargo.toml")
        self.assertEqual(read_json("package.json")["version"], version)
        # Kumi's host for REAPER and Renoise ships beside it and moves with it (its MCP serverInfo and hello say it).
        for name in ("crates/kumi-common/Cargo.toml", "crates/kumi-runtime/Cargo.toml", "crates/daw-mcp-server/Cargo.toml"):
            self.assertEqual(cargo_version(name), version, name)
        declared = re.search(r'KUMI_VERSION[^=]*=\s*"([^"\n]+)"', (root / "crates/kumi-runtime/src/version.rs").read_text())
        self.assertIsNotNone(declared)
        self.assertEqual(declared[1], version)
        cargo_lock = tomllib.loads((root / "Cargo.lock").read_text())["package"]
        for package in cargo_lock:
            if package["name"] in ("kumi", "kumi-runtime", "kumi-common", "daw-mcp-server"):
                self.assertEqual(package["version"], version, package["name"])
        bridge = cargo_version("crates/ableton-mcp-server/Cargo.toml")
        self.assertEqual(next(p["version"] for p in cargo_lock if p["name"] == "ableton-mcp-server"), bridge)
        # REAPER's extension and Renoise's tool: each version a release can bump, said once (and Renoise's manifest
        # number agreeing with its version file).
        reaper = cargo_version("crates/reaper-kumi/Cargo.toml")
        self.assertRegex(reaper, r"^\d+\.\d+\.\d+$")
        self.assertEqual(next(p["version"] for p in cargo_lock if p["name"] == "reaper-kumi"), reaper)
        self.assertEqual(release.renoise_tool(root)[1], re.search(r'return "([^"]+)"', (root / release.RENOISE_TOOL_SOURCE / "payload/version.lua").read_text())[1])
        # Logic's bridge: the version file the Apple Silicon Mac's bundle carries is its crate's, as Cargo.lock has it (and as
        # the binary says it: logic-kumi's BRIDGE_VERSION is its package version).
        logic = cargo_version(release.LOGIC_CRATE + "/Cargo.toml")
        self.assertRegex(logic, r"^\d+\.\d+\.\d+$")
        self.assertEqual(next(p["version"] for p in cargo_lock if p["name"] == "logic-kumi"), logic)
        self.assertEqual(release.crate_version(root, release.LOGIC_CRATE), logic)
        self.assertIn('BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION")', (root / release.LOGIC_CRATE / "src/lib.rs").read_text())

    def helper_source(self):
        source = self.root / "crates/kumi-runtime/src/integrations/ableton/hands/KumiHands.swift"
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
            # Others' code Kumi carries is under their licenses, said beside Kumi's own.
            self.assertIn("THIRD_PARTY_NOTICES.md", names)
            helper_name = "kumi-hands-" + release.digest(release.ROOT / "crates/kumi-runtime/src/integrations/ableton/hands/KumiHands.swift")[:12]
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
            self.assertEqual(manifest["protocol"]["registryHash"], "dde0289832e75a26b909933099c9adcae4815a157e1b9835f8feb32de9ed50b5")
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
        """vendor/willington in the fixture root, as a Willington update writes it (replacing any earlier one)."""
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
        # Willington updates change vendor/willington: CI checks them here, before a release ships them.
        release.willington_files(release.ROOT / release.WILLINGTON)

    def willington_bundle(self, files, links=()):
        """A Willington-matrix.zip holding these files, named exactly as given; links are symlinks."""
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive, warnings.catch_warnings():
            warnings.simplefilter("ignore")  # a name twice
            for name, content in [*files.items(), *((name, b"target") for name in links)]:
                info = zipfile.ZipInfo("placeholder")
                info.filename = name  # as given, even where this system's zipfile would rewrite it
                if name in links:
                    info.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(info, content)
        return buffer.getvalue()

    def bundle_run(self, files, run=None, compare="behind", artifacts=None, sidecar=None, root=("LICENSE.md", "README.md")):
        """GitHub's API, through gh, for Bundle run 7 of these files on a push to Willington's main; the arguments
        change its answers. A path it doesn't know fails as an HTTP error does."""
        repository, commit = vendor.REPOSITORY, "c" * 40
        bundle = self.willington_bundle(files)
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("Willington-matrix.zip", bundle)
            archive.writestr("Willington-matrix.zip.sha256", sidecar or hashlib.sha256(bundle).hexdigest() + "  Willington-matrix.zip\n")
        digest = "sha256:" + hashlib.sha256(buffer.getvalue()).hexdigest()
        answers = {
            f"repos/{repository}/actions/runs/7": {"event": "push", "head_branch": "main", "path": vendor.WORKFLOW, "status": "completed",
                "conclusion": "success", "head_repository": {"full_name": repository}, "repository": {"full_name": repository},
                "head_sha": commit, "html_url": f"https://github.com/{repository}/actions/runs/7", **(run or {})},
            f"repos/{repository}/compare/main...{commit}": {"status": compare},
            f"repos/{repository}/actions/runs/7/artifacts?per_page=100": {"artifacts": artifacts if artifacts is not None else [
                {"id": 9, "name": "Willington-matrix", "digest": digest, "expired": False},
                {"id": 8, "name": "windows-artifacts", "digest": "sha256:" + "0" * 64, "expired": False}]},
            f"repos/{repository}/actions/artifacts/9/zip": buffer.getvalue(),
            f"repos/{repository}/contents?ref={commit}": [{"name": name, "type": "file"} for name in root],
            f"repos/{repository}/contents/LICENSE.md?ref={commit}": b"Willington license fixture\n",
        }
        def api(path, *headers):
            if path not in answers:
                raise RuntimeError(f"gh api {path} failed: Not Found (HTTP 404)")
            answer = answers[path]
            return answer if isinstance(answer, bytes) else json.dumps(answer).encode()
        return api

    def runtime_files(self):
        _, files = self.vendor_willington()
        return {name: content for name, content in files.items() if name != "LICENSE"}

    def test_an_update_puts_a_bundle_runs_files_and_license_in_vendor_willington(self):
        folder = self.root / release.WILLINGTON
        runtime = self.runtime_files()
        fetched = vendor.fetch(7, self.bundle_run(runtime))
        self.assertEqual((fetched["commit"], fetched["artifact"]), ("c" * 40, 9))
        self.assertEqual(fetched["licenses"], {"LICENSE.md": b"Willington license fixture\n"})
        listed = vendor.vendor(fetched["bundle"], fetched["licenses"], fetched["commit"], root=self.root)
        self.assertEqual(set(listed), {*runtime, "LICENSE.md"})
        self.assertEqual(release.willington_files(folder), listed)
        expected_manifest = {"schema": release.WILLINGTON_SCHEMA, "version": "c" * 12, "commit": "c" * 40, "files": listed}
        # --check compares bytes; a Windows import must not translate these LF bytes to CRLF.
        self.assertEqual((folder / "release.json").read_bytes(),
                         (json.dumps(expected_manifest, indent=2, sort_keys=True) + "\n").encode("utf-8"))
        # The earlier update's files go, its LICENSE among them, and nothing staged is left beside the folder.
        self.assertFalse((folder / "LICENSE").exists())
        self.assertEqual([path.name for path in folder.parent.iterdir()], ["willington"])
        # A reviewer's check rebuilds the folder from the run and finds each changed file.
        self.assertEqual(vendor.check(fetched, root=self.root), [])
        (folder / "WillingtonRuntime/matrix.json").write_bytes(b'{"changed": true}\n')
        (folder / "WillingtonRuntime/extra.py").write_bytes(b"")
        self.assertEqual(vendor.check(fetched, root=self.root), ["WillingtonRuntime/extra.py", "WillingtonRuntime/matrix.json"])
        vendor.vendor(fetched["bundle"], fetched["licenses"], fetched["commit"], "0.5.0", root=self.root)
        self.assertEqual(json.loads((folder / "release.json").read_text(encoding="utf-8"))["version"], "0.5.0")
        self.assertEqual(vendor.check(fetched, root=self.root), [])
        self.assertEqual(vendor.check({**fetched, "commit": "e" * 40}, root=self.root), ["release.json"])

    def test_editing_runtime_import_and_platform_filter_keep_provenance_checks(self):
        runtime = self.runtime_files()
        library = "WillingtonEditing/build/live-12.4.15b5-editing-arm64/libwillington_editing.dylib"
        runtime.update({"WillingtonEditing/__init__.py": b"", "WillingtonEditing/api.py": b"# fixture\n",
                        library: b"native fixture", "WillingtonEditing/build/live-12.4.15b5-editing-arm64/build.json": b"{}"})
        fetched = vendor.fetch(7, self.bundle_run(runtime))
        vendor.vendor(fetched["bundle"], fetched["licenses"], fetched["commit"], root=self.root)
        self.assertEqual(vendor.check(fetched, root=self.root), [])
        for target, expected in [("aarch64-apple-darwin", True), ("x86_64-pc-windows-msvc", False)]:
            remote = self.root / target / "AbletonMcpBridge"
            release.stage_willington(self.root, remote, target)
            self.assertEqual((remote / "willington" / library).exists(), expected)
        (self.root / release.WILLINGTON / library).write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            release.willington_files(self.root / release.WILLINGTON)

    def test_only_a_successful_bundle_run_on_a_push_to_willingtons_main_is_taken(self):
        runtime = self.runtime_files()
        listed = {"id": 9, "name": "Willington-matrix", "digest": "sha256:" + "0" * 64, "expired": False}
        for changes, refused in [
                ({"run": {"event": "pull_request"}}, "event 'pull_request'"),
                ({"run": {"event": "workflow_dispatch"}}, "event 'workflow_dispatch'"),
                ({"run": {"head_branch": "ci/bundle"}}, "head_branch 'ci/bundle'"),
                # A fork's branch called main.
                ({"run": {"head_repository": {"full_name": "someone/willington"}}}, "head_repository 'someone/willington'"),
                ({"run": {"path": ".github/workflows/verify.yml"}}, "path"),
                ({"run": {"status": "in_progress", "conclusion": None}}, "status 'in_progress', conclusion None"),
                ({"run": {"conclusion": "failure"}}, "conclusion 'failure'"),
                ({"run": {"head_sha": "main"}}, "names no commit"),
                ({"compare": "ahead"}, "isn't on xonedsp/willington's main \\(ahead\\)"),
                ({"compare": "diverged"}, "isn't on"),
                ({"artifacts": []}, "0 Willington-matrix artifacts"),
                ({"artifacts": [listed, {**listed, "id": 10}]}, "2 Willington-matrix artifacts"),
                ({"artifacts": [{**listed, "expired": True}]}, "expired: rerun it"),
                ({"artifacts": [listed]}, "not the sha256:0+ GitHub recorded"),
                ({"sidecar": "0" * 64 + "  Willington-matrix.zip\n"}, "doesn't match its .sha256"),
                ({"root": ("README.md",)}, "has no LICENSE or LICENSE.md")]:
            with self.subTest(changes=changes):
                with self.assertRaisesRegex(ValueError, refused):
                    vendor.fetch(7, self.bundle_run(runtime, **changes))
        # A license gh can't fetch fails the update rather than becoming its error page.
        with self.assertRaisesRegex(RuntimeError, "LICENSE\\?ref=.*HTTP 404"):
            vendor.fetch(7, self.bundle_run(runtime, root=("LICENSE",)))

    def test_the_update_script_says_why_it_stops_in_one_line(self):
        for error in (ValueError("run 7 isn't a successful Bundle run"), RuntimeError("gh api x failed: Not Found (HTTP 404)")):
            with self.subTest(error=error), patch.object(vendor, "fetch", side_effect=error), \
                    patch.object(sys, "argv", ["vendor-willington.py", "--run", "7"]):
                with self.assertRaises(SystemExit) as stopped:
                    vendor.main()
                self.assertEqual(stopped.exception.code, str(error))
        # A tree without an update has nothing to check against.
        with self.assertRaisesRegex(ValueError, "has no release.json with a version to check against"):
            vendor.check({}, root=self.root)

    def test_a_refused_update_leaves_vendor_willington_as_it_was(self):
        folder, files = self.vendor_willington()
        before = release.inventory(folder)
        runtime = {name: content for name, content in files.items() if name != "LICENSE"}
        licenses = {"LICENSE.md": b"Willington license fixture\n"}
        for extra, refused in [({"../outside.py": b""}, "can't go in"), ({"/absolute.py": b""}, "can't go in"),
                               ({"WillingtonRuntime/../../outside.py": b""}, "can't go in"),
                               ({"WillingtonRuntime\\x.py": b""}, "can't go in"), ({"release.json": b"{}"}, "can't go in"),
                               ({"LICENSE": b"another notice\n"}, "can't go in"), ({"NOTICE.md": b"fixture"}, "can't go in"),
                               ({"willingtonruntime/x.py": b""}, "can't go in"),
                               ({"WillingtonRuntime/Matrix.json": b"{}\n"}, "differ only in case"),
                               ({"WillingtonDeviceTools/native_windows.cpp": b"fixture"}, "isn't one of Willington's runtime files")]:
            with self.subTest(extra=extra):
                with self.assertRaisesRegex(ValueError, refused):
                    vendor.vendor(self.willington_bundle({**runtime, **extra}), licenses, "d" * 40, root=self.root)
                self.assertEqual(release.inventory(folder), before)
        with self.assertRaisesRegex(ValueError, "can't go in"):
            vendor.vendor(self.willington_bundle(runtime, links=["WillingtonRuntime/link.py"]), licenses, "d" * 40, root=self.root)
        bundle = io.BytesIO(self.willington_bundle(runtime))
        with zipfile.ZipFile(bundle, "a") as archive, warnings.catch_warnings():
            warnings.simplefilter("ignore")
            archive.writestr("WillingtonRuntime/matrix.json", b'{"second": true}\n')
        with self.assertRaisesRegex(ValueError, "twice"):
            vendor.vendor(bundle.getvalue(), licenses, "d" * 40, root=self.root)
        with self.assertRaisesRegex(ValueError, "invalid commit"):
            vendor.vendor(self.willington_bundle(runtime), licenses, "main", root=self.root)
        with self.assertRaisesRegex(ValueError, "license notice"):
            vendor.vendor(self.willington_bundle(runtime), {"COPYING": b"Willington license fixture\n"}, "d" * 40, root=self.root)
        self.assertEqual(release.inventory(folder), before)
        self.assertEqual([path.name for path in folder.parent.iterdir()], ["willington"])

    def library(self, data, name="library"):
        path = self.root / name
        path.write_bytes(data)
        return path

    def test_each_library_format_gives_its_architectures_exports_and_loader_facts(self):
        elf = release.binary_facts(self.library(elf_library(183, glibc=("2.17", "2.34", "2.28"))))
        self.assertEqual(elf, [{"arch": "arm64", "exports": {"ReaperPluginEntry"}, "glibc": (2, 34)}])
        mac = release.binary_facts(self.library(universal(macho_library(0x01000007), macho_library(exports=("ReaperPluginEntry", "Other")))))
        self.assertEqual([(piece["arch"], piece["exports"], piece["install_name"], piece["signed"]) for piece in mac],
                         [("x86_64", {"ReaperPluginEntry"}, release.REAPER_INSTALL_NAME, True),
                          ("arm64", {"ReaperPluginEntry", "Other"}, release.REAPER_INSTALL_NAME, True)])
        pe = release.binary_facts(self.library(pe_library(imports=("KERNEL32.dll", "VCRUNTIME140.dll"), delayed=("bcrypt.dll",))))
        self.assertEqual(pe, [{"arch": "x86_64", "exports": {"ReaperPluginEntry"}, "imports": {"kernel32.dll", "vcruntime140.dll", "bcrypt.dll"}}])
        for junk in (b"#!/bin/sh\n", b"\x7fELF\x02\x01" + b"\0" * 10, b"MZ" + b"\0" * 62, b"\xca\xfe\xba\xbe\0\0\0\x01"):
            with self.subTest(junk=junk), self.assertRaises(ValueError):
                release.binary_facts(self.library(junk))

    def test_the_extension_is_checked_as_each_systems_reaper_loads_it(self):
        mac = universal(macho_library(0x01000007), macho_library())
        for target, data in [("x86_64-unknown-linux-gnu", elf_library()), ("aarch64-unknown-linux-gnu", elf_library(183)),
                             ("aarch64-apple-darwin", mac), ("x86_64-apple-darwin", mac),
                             ("x86_64-pc-windows-msvc", pe_library()), ("aarch64-pc-windows-msvc", pe_library())]:
            with self.subTest(target=target):
                checked = release.check_extension(self.library(data, release.extension_name(target)), target)
                self.assertEqual(checked["archs"], release.extension_archs(target))
        built = "/Users/runner/work/kumi/kumi/target/x86_64-apple-darwin/release/deps/libreaper_kumi.dylib"
        for target, data, said in [
                # One architecture: a REAPER under Rosetta on an Apple silicon Mac wouldn't load it.
                ("aarch64-apple-darwin", macho_library(), "holds arm64, not arm64, x86_64"),
                ("aarch64-apple-darwin", universal(macho_library(0x01000007, signed=False), macho_library()), "(x86_64) isn't signed"),
                ("x86_64-apple-darwin", universal(macho_library(0x01000007, install_name=built), macho_library()), f"names itself {built}"),
                ("x86_64-unknown-linux-gnu", elf_library(exports=("SWELL_dllMain",)), "doesn't export ReaperPluginEntry"),
                ("x86_64-unknown-linux-gnu", elf_library(glibc=("2.2.5", "2.38")), "needs glibc 2.38, newer than Kumi's 2.35"),
                ("x86_64-unknown-linux-gnu", elf_library(183), "holds arm64, not x86_64"),
                ("x86_64-pc-windows-msvc", pe_library(imports=("KERNEL32.dll", "VCRUNTIME140.dll", "api-ms-win-crt-runtime-l1-1-0.dll")),
                 "needs the C runtime installed (api-ms-win-crt-runtime-l1-1-0.dll, vcruntime140.dll)"),
                ("x86_64-pc-windows-msvc", pe_library(delayed=("ucrtbase.dll",)), "needs the C runtime installed (ucrtbase.dll)"),
                # Windows on Arm gets the x64 build: REAPER's Arm build is ARM64EC, which loads x64 libraries.
                ("aarch64-pc-windows-msvc", pe_library(machine=0xAA64), "holds arm64, not x86_64")]:
            with self.subTest(said=said), self.assertRaisesRegex(ValueError, re.escape(said)):
                release.check_extension(self.library(data, release.extension_name(target)), target)
        # CI's REAPER job checks its one build of the runner's own architecture.
        release.check_extension(self.library(macho_library(), "thin.dylib"), "aarch64-apple-darwin", ["arm64"])
        with self.assertRaisesRegex(ValueError, "no REAPER extension is built for x86_64-unknown-linux-musl"):
            release.extension_name("x86_64-unknown-linux-musl")

    def test_the_extension_is_built_as_it_ships_on_each_system(self):
        calls = []
        def run(command, cwd=None, env=None, check=None):
            calls.append((command[0], command[1:], (env or {}).get("RUSTFLAGS")))
            if command[0] == "cargo":
                folder, target = Path(command[command.index("--target-dir") + 1]), command[command.index("--target") + 1]
                built = folder / target / "ci-release" / release.cargo_library(target)
                built.parent.mkdir(parents=True, exist_ok=True)
                built.write_bytes(target.encode())
            elif command[0] == "lipo":
                Path(command[command.index("-output") + 1]).write_bytes(b"universal")
        target_dir = self.root / "target"
        with patch.object(release.subprocess, "run", side_effect=run), patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target_dir)}):
            os.environ.pop("RUSTFLAGS", None)
            for target in ("x86_64-apple-darwin", "aarch64-pc-windows-msvc", "aarch64-unknown-linux-gnu"):
                final, commands = release.build_extension(self.root, target, "ci-release", self.root / target)
                self.assertEqual(final, self.root / target / release.extension_name(target))
                self.assertEqual(len(commands), len(release.extension_builds(target)))
        cargo = lambda target, folder: ("cargo", ["build", "--locked", "--profile", "ci-release", "--target", target, "-p", "reaper-kumi",
                                                  "--target-dir", str(folder)])
        mac = self.root / "x86_64-apple-darwin" / "reaper_kumi.dylib"
        self.assertEqual([call[:2] for call in calls[:5]], [
            cargo("aarch64-apple-darwin", target_dir), cargo("x86_64-apple-darwin", target_dir),
            ("lipo", ["-create", "-output", str(mac), str(target_dir / "aarch64-apple-darwin/ci-release/libreaper_kumi.dylib"),
                      str(target_dir / "x86_64-apple-darwin/ci-release/libreaper_kumi.dylib")]),
            # The platform's strip, then a signature over what lipo and strip made.
            ("strip", ["-x", str(mac)]), ("codesign", ["--force", "--sign", "-", str(mac)])])
        # Windows, on either architecture: the x64 build, the C runtime linked in, in a target folder of its own.
        self.assertEqual(calls[5], (*cargo("x86_64-pc-windows-msvc", target_dir / "crt-static"), "-C target-feature=+crt-static"))
        self.assertEqual((self.root / "aarch64-pc-windows-msvc/reaper_kumi.dll").read_bytes(), b"x86_64-pc-windows-msvc")
        self.assertEqual(calls[6:], [(*cargo("aarch64-unknown-linux-gnu", target_dir), None),
                                     ("strip", ["--strip-unneeded", str(self.root / "aarch64-unknown-linux-gnu/reaper_kumi.so")], None)])
        # Never a rustc strip (on macOS 27 it can leave a library dyld refuses): the platform's strip above instead.
        profiles = tomllib.loads((release.ROOT / "Cargo.toml").read_text())["profile"]
        for profile in ("dev", "release", "ci-release"):
            self.assertEqual(profiles[profile]["package"]["reaper-kumi"]["strip"], "none", profile)

    def test_the_bundle_carries_reapers_extension_and_renoises_tool_where_kumi_installs_them_from(self):
        out = self.root / "release"
        result = self.build(out)
        tool = release.ROOT / release.RENOISE_TOOL_SOURCE
        reaper = release.crate_version(release.ROOT, release.REAPER_CRATE)
        renoise = re.search(r'return "([^"]+)"', (tool / "payload/version.lua").read_text())[1]
        self.assertEqual((result["reaper"], result["renoise"]), (reaper, renoise))
        with tarfile.open(out / result["bundle"]) as tar:
            names = set(tar.getnames())
            self.assertIn("daw-mcp-server", names)
            self.assertEqual(tar.extractfile("daws/reaper/reaper_kumi.so").read(), self.extension.read_bytes())
            if EXEC_BITS:
                self.assertEqual(tar.getmember("daws/reaper/reaper_kumi.so").mode, 0o755)
            self.assertEqual(tar.extractfile("daws/reaper/reaper_kumi.version").read(), f"{reaper}\n".encode())
            files = release.inventory(tool)
            prefix = "daws/renoise/com.kumi.Bridge.xrnx/"
            self.assertEqual({name for name in names if name.startswith("daws/")},
                             {"daws/reaper/reaper_kumi.so", "daws/reaper/reaper_kumi.version", *(prefix + name for name in files)})
            for name in files:
                self.assertEqual(tar.extractfile(prefix + name).read(), (tool / name).read_bytes(), name)
            # `bridge` stays Live's, as every earlier Kumi and installer reads it: REAPER's and Renoise's are new keys. Linux
            # has no Logic Pro, so its bundle carries no bridge for it and says none.
            for name in ("package.json", "kumi-install.json"):
                metadata = json.load(tar.extractfile(name))
                self.assertEqual((metadata["bridge"], metadata["reaper"], metadata["renoise"]), (result["bridge"], reaper, renoise), name)
                self.assertNotIn("logic", metadata, name)
            self.assertFalse([name for name in names if "logic" in name])
            # install.sh reads the first "version" in package.json as Kumi's.
            versions = [line for line in tar.extractfile("package.json").read().decode().splitlines() if '"version"' in line]
            self.assertEqual(versions, [f'  "version": "{result["kumi"]}",'])
        self.assertEqual(json.loads((out / "kumi-release.json").read_text())["reaper"], reaper)
        self.assertNotIn("logic", json.loads((out / "kumi-release.json").read_text()))

    def test_a_bundle_carries_a_checked_extension_or_none_is_built(self):
        with self.assertRaisesRegex(ValueError, "a bundle carries REAPER's extension"):
            release.build_release(release.ROOT, self.binaries, self.root / "none", "x86_64-unknown-linux-gnu", self.source, self.builder, "fixture")
        for name in release.BINARIES:
            (self.binaries / (name + ".exe")).write_bytes(b"MZ fixture " + name.encode())
        dll = self.library(pe_library(imports=("KERNEL32.dll", "VCRUNTIME140.dll")), "reaper_kumi.dll")
        out = self.root / "windows"
        with self.assertRaisesRegex(ValueError, "build it with \\+crt-static"):
            release.build_release(release.ROOT, self.binaries, out, "x86_64-pc-windows-msvc", self.source, self.builder, "fixture",
                                  reaper_extension=dll)
        self.assertEqual(list(out.iterdir()), [])
        dll.write_bytes(pe_library())
        result = release.build_release(release.ROOT, self.binaries, out, "x86_64-pc-windows-msvc", self.source, self.builder, "fixture",
                                       reaper_extension=dll)
        with tarfile.open(out / result["bundle"]) as tar:
            self.assertEqual(tar.extractfile("daws/reaper/reaper_kumi.dll").read(), dll.read_bytes())
            self.assertIn("daw-mcp-server.exe", tar.getnames())
            # REAPER's and Renoise's, and nothing of Logic Pro's, which Windows doesn't run.
            prefix = "daws/renoise/com.kumi.Bridge.xrnx/"
            self.assertEqual({name for name in tar.getnames() if name.startswith("daws/")},
                             {"daws/reaper/reaper_kumi.dll", "daws/reaper/reaper_kumi.version",
                              *(prefix + name for name in release.inventory(release.ROOT / release.RENOISE_TOOL_SOURCE))})
            self.assertFalse([name for name in tar.getnames() if "logic" in name])
            for name in ("package.json", "kumi-install.json"):
                self.assertNotIn("logic", json.load(tar.extractfile(name)), name)
        self.assertNotIn("logic", result)

    def logic_checkout(self):
        """A checkout with this one's Logic crate manifest and helper source, and that helper built as
        scripts/build-logic-hands.py leaves it (universal, signed): the checkout, the helper's name and the helper."""
        checkout = self.root / "checkout"
        for name in (release.LOGIC_CRATE + "/Cargo.toml", release.LOGIC_HELPER_SOURCE):
            (checkout / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(release.ROOT / name, checkout / name)
        name = release.logic_helper_name(checkout)
        helper = checkout / release.LOGIC_HANDS / name
        helper.parent.mkdir(parents=True)
        helper.write_bytes(universal(macho_library(0x01000007), macho_library()))
        # The bridge as cargo builds it for Apple Silicon: arm64, signed by the linker.
        (self.binaries / release.LOGIC_BRIDGE).write_bytes(macho_library())
        return checkout, name, helper

    def test_logics_bridge_is_staged_flat_with_the_current_helper_and_the_crates_version(self):
        checkout, name, helper = self.logic_checkout()
        bundle = self.root / "bundle"
        version = release.stage_logic(checkout, bundle, "aarch64-apple-darwin", self.binaries)
        crate = release.crate_version(release.ROOT, release.LOGIC_CRATE)
        self.assertEqual(version, crate)
        # Where Kumi sets it up from (own.rs: daws/logic/logic-kumi, its `version` read as the bridge's): the bridge, the
        # helper and the version, and nothing else.
        folder = bundle / "daws/logic/logic-kumi"
        self.assertEqual(release.inventory(folder), {"logic-kumi": release.digest(self.binaries / "logic-kumi"),
                                                     name: release.digest(helper), "version": release.digest(folder / "version")})
        self.assertEqual((folder / "version").read_bytes(), f"{crate}\n".encode())
        if EXEC_BITS:
            modes = {path.name: path.stat().st_mode & 0o777 for path in folder.iterdir()}
            self.assertEqual(modes, {"logic-kumi": 0o755, name: 0o755, "version": 0o644})
        # The helper's name is the one scripts/build-logic-hands.py gives its build of this source, in the folder it builds in.
        spec = importlib.util.spec_from_file_location("build_logic_hands", release.ROOT / "scripts/build-logic-hands.py")
        hands = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(hands)
        self.assertEqual(hands.helper_name(checkout / release.LOGIC_HELPER_SOURCE), name)
        self.assertEqual(hands.OUT.relative_to(hands.ROOT).as_posix(), release.LOGIC_HANDS)
        self.assertEqual(hands.SOURCE.relative_to(hands.ROOT).as_posix(), release.LOGIC_HELPER_SOURCE)
        # Only the Apple Silicon Mac's bundle carries it: the others don't look for the helper, nor the bridge.
        for target in ("x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc",
                       "aarch64-pc-windows-msvc"):
            with self.subTest(target=target):
                self.assertIsNone(release.stage_logic(self.root / "nowhere", self.root / target, target, self.root / "nothing"))
                self.assertFalse((self.root / target).exists())

    def test_logics_bridge_ships_only_the_current_helper_and_a_bridge_apple_silicon_runs(self):
        checkout, name, helper = self.logic_checkout()
        hands = helper.parent
        stage = lambda: release.stage_logic(checkout, self.root / "bundle", "aarch64-apple-darwin", self.binaries)
        built = helper.read_bytes()
        helper.unlink()
        with self.assertRaisesRegex(ValueError, "run python3 scripts/build-logic-hands.py first"):
            stage()
        # An older source's build, alone or beside the current one, and a build left half done.
        for others in (["kumi-logic-hands-000000000000"], ["kumi-logic-hands-000000000000", name], [name + ".partial", name]):
            with self.subTest(others=others):
                for other in others:
                    (hands / other).write_bytes(built)
                with self.assertRaisesRegex(ValueError, f"build of the current source alone, {name}, not "):
                    stage()
                for other in others:
                    (hands / other).unlink()
        helper.write_bytes(built)
        stage()
        # What Apple Silicon wouldn't run: a bridge built for another system or architecture, a helper not signed.
        for bridge, data, said in [(True, elf_library(183), "logic-kumi isn't a Mach-O"),
                                   (True, macho_library(0x01000007), "logic-kumi holds x86_64, not arm64"),
                                   (True, macho_library(signed=False), "logic-kumi (arm64) isn't signed"),
                                   (False, universal(macho_library(0x01000007), macho_library(signed=False)), f"{name} (arm64) isn't signed")]:
            with self.subTest(said=said):
                (self.binaries / "logic-kumi").write_bytes(macho_library())
                helper.write_bytes(built)
                (self.binaries / "logic-kumi" if bridge else helper).write_bytes(data)
                with self.assertRaisesRegex(ValueError, re.escape(said)):
                    stage()
        (self.binaries / "logic-kumi").write_bytes(macho_library())
        helper.write_bytes(built)
        # A link, for the helper or the bridge: the lifecycle refuses links, so the bundle never holds one.
        target = self.root / "linked"
        target.write_bytes(built)
        helper.unlink()
        try:
            helper.symlink_to(target)
        except OSError:
            return  # Windows runners may not grant symlink creation.
        with self.assertRaisesRegex(ValueError, "link or special file"):
            stage()
        helper.unlink()
        helper.write_bytes(built)
        (self.binaries / "logic-kumi").unlink()
        (self.binaries / "logic-kumi").symlink_to(target)
        with self.assertRaisesRegex(ValueError, "payload must be a regular file"):
            stage()

    def test_the_apple_silicon_macs_bundle_carries_logics_bridge_and_the_intel_macs_none(self):
        checkout, name, helper = self.logic_checkout()
        extension = self.library(universal(macho_library(0x01000007), macho_library()), "reaper_kumi.dylib")
        crate = release.crate_version(release.ROOT, release.LOGIC_CRATE)
        tool = release.inventory(release.ROOT / release.RENOISE_TOOL_SOURCE)
        staged = release.stage_logic
        # Live's helper is stage_hands' (tested above); Logic's comes from the checkout made here.
        with patch.object(release, "stage_hands"), \
                patch.object(release, "stage_logic", side_effect=lambda root, *rest: staged(checkout, *rest)):
            for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
                out = self.root / target
                result = release.build_release(release.ROOT, self.binaries, out, target, self.source, self.builder, "fixture",
                                               reaper_extension=extension)
                with tarfile.open(out / result["bundle"]) as tar:
                    names = set(tar.getnames())
                    daws = {"daws/reaper/reaper_kumi.dylib", "daws/reaper/reaper_kumi.version",
                            *("daws/renoise/com.kumi.Bridge.xrnx/" + file for file in tool)}
                    logic = {f"daws/logic/logic-kumi/{file}" for file in ("logic-kumi", name, "version")}
                    metadata = [json.load(tar.extractfile(file)) for file in ("package.json", "kumi-install.json")]
                    if target == "aarch64-apple-darwin":
                        self.assertEqual({file for file in names if file.startswith("daws/")}, daws | logic)
                        self.assertEqual(tar.extractfile("daws/logic/logic-kumi/logic-kumi").read(), (self.binaries / "logic-kumi").read_bytes())
                        self.assertEqual(tar.extractfile(f"daws/logic/logic-kumi/{name}").read(), helper.read_bytes())
                        self.assertEqual(tar.extractfile("daws/logic/logic-kumi/version").read(), f"{crate}\n".encode())
                        if EXEC_BITS:
                            self.assertEqual([tar.getmember(file).mode for file in sorted(logic)], [0o755, 0o755, 0o644])
                        # Kumi sets it up from daws/logic alone: the host runs the copy in Kumi's folder, never one beside Kumi.
                        self.assertNotIn("logic-kumi", names)
                        self.assertEqual([value.get("logic") for value in metadata] + [result.get("logic")], [crate] * 3)
                        self.assertEqual(json.loads((out / "kumi-release.json").read_text())["logic"], crate)
                    else:
                        # Logic Pro 12 runs on Apple Silicon only: the Intel Mac's bundle carries no bridge for it, and says none.
                        self.assertEqual({file for file in names if file.startswith("daws/")}, daws)
                        self.assertFalse([file for file in names if "logic" in file])
                        self.assertEqual([value.get("logic", "none") for value in metadata] + [result.get("logic", "none")], ["none"] * 3)

    def test_the_aggregate_refuses_targets_whose_daw_bridges_differ(self):
        spec = importlib.util.spec_from_file_location("migration_release", release.ROOT / "scripts/build-migration-release.py")
        migration = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(migration)
        manifests = []
        for target, reaper in (("x86_64-unknown-linux-gnu", "0.1.0"), ("aarch64-unknown-linux-gnu", "0.1.1")):
            out = self.root / target
            out.mkdir()
            (out / "kumi.tar.gz").write_bytes(target.encode())
            manifest = {"kumi": "1.20.0", "bundle": "kumi.tar.gz", "sha256": release.digest(out / "kumi.tar.gz"), "runtime": "rust-native",
                        "target": target, "bridge": "1.0.93", "reaper": reaper, "renoise": "1.0.0"}
            (out / "kumi-release.json").write_text(json.dumps(manifest))
            manifests.append(out / "kumi-release.json")
        with self.assertRaisesRegex(ValueError, "same Kumi, bridge, REAPER extension and Renoise tool versions"):
            migration.build(manifests, self.root / "aggregate", "24.0.0")
        same = json.loads(manifests[1].read_text())
        manifests[1].write_text(json.dumps({**same, "reaper": "0.1.0"}))
        self.assertEqual(migration.build(manifests, self.root / "aggregate", "24.0.0")["bridge"], "1.0.93")

    def test_the_aggregate_compares_logics_bridge_among_the_macs_that_carry_it(self):
        spec = importlib.util.spec_from_file_location("migration_release", release.ROOT / "scripts/build-migration-release.py")
        migration = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(migration)
        def manifests(logic: dict[str, str | None]) -> list[Path]:
            """One commit's target manifests, each target's Logic bridge as given (None: it carries none)."""
            folder = Path(tempfile.mkdtemp(dir=self.root))
            paths = []
            for target, version in logic.items():
                out = folder / target
                out.mkdir()
                (out / "kumi.tar.gz").write_bytes(target.encode())
                manifest = {"kumi": "1.20.0", "bundle": "kumi.tar.gz", "sha256": release.digest(out / "kumi.tar.gz"), "runtime": "rust-native",
                            "target": target, "bridge": "1.0.93", "reaper": "0.1.0", "renoise": "1.0.0",
                            **({"logic": version} if version else {})}
                (out / "kumi-release.json").write_text(json.dumps(manifest))
                paths.append(out / "kumi-release.json")
            return paths
        everyone = {"aarch64-apple-darwin": "0.1.0", "x86_64-apple-darwin": None, "x86_64-unknown-linux-gnu": None,
                    "aarch64-pc-windows-msvc": None}
        built = migration.build(manifests(everyone), self.root / "aggregate", "24.0.0")
        self.assertEqual({target: value.get("logic") for target, value in built["targets"].items()}, everyone)
        # Only the Apple Silicon Mac's bundle carries it: one elsewhere is a bundle built some other way.
        for target in ("x86_64-apple-darwin", "x86_64-unknown-linux-gnu"):
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, f"only aarch64-apple-darwin carries Logic Pro's bridge, not {target}"):
                migration.build(manifests({**everyone, target: "0.1.0"}), self.root / "refused", "24.0.0")
        # Among the Macs that carry it, one version, as for every other bridge.
        with patch.object(migration.native, "LOGIC_TARGETS", ("aarch64-apple-darwin", "x86_64-apple-darwin")):
            with self.assertRaisesRegex(ValueError, "the Mac targets that carry Logic Pro's bridge must have the same version of it"):
                migration.build(manifests({**everyone, "x86_64-apple-darwin": "0.1.1"}), self.root / "refused", "24.0.0")
            migration.build(manifests({**everyone, "x86_64-apple-darwin": "0.1.0"}), self.root / "aggregate", "24.0.0")

    def test_renoise_tool_ships_only_its_own_files_with_one_version(self):
        root = self.root / "checkout"
        tool = root / release.RENOISE_TOOL_SOURCE
        shutil.copytree(release.ROOT / release.RENOISE_TOOL_SOURCE, tool)
        release.renoise_tool(root)
        manifest = (tool / "manifest.xml").read_text()
        for extra, said in [("preferences.xml", "preferences.xml isn't one of the tool's own files"),
                            ("bridge-reference.json", "bridge-reference.json isn't one of"),
                            ("payload/.DS_Store", "payload/.DS_Store isn't one of"), ("payload/notes.txt", "payload/notes.txt isn't one of")]:
            with self.subTest(extra=extra), self.assertRaisesRegex(ValueError, re.escape(said)):
                (tool / extra).write_text("x")
                try:
                    release.renoise_tool(root)
                finally:
                    (tool / extra).unlink()
        version = re.search(r"<Version>([^<]+)</Version>", manifest)[1]
        for changed, said in [(manifest.replace(f"<Version>{version}</Version>", "<Version>9.99</Version>"), "manifest says Version 9.99"),
                              (manifest.replace("<Id>com.kumi.Bridge</Id>", "<Id>com.other.Tool</Id>"), "names com.other.Tool"),
                              (manifest.replace("<ApiVersion>6.2</ApiVersion>", "<ApiVersion>6.1</ApiVersion>"), "declares API 6.1")]:
            with self.subTest(said=said), self.assertRaisesRegex(ValueError, re.escape(said)):
                (tool / "manifest.xml").write_text(changed)
                try:
                    release.renoise_tool(root)
                finally:
                    (tool / "manifest.xml").write_text(manifest)
        # Renoise compares the manifest's Version as a number: each later version is a larger one.
        ordered = ["1.0.0", "1.0.1", "1.0.12", "1.1.0", "1.1.5", "1.12.0", "2.0.0", "2.10.15"]
        numbers = [release.renoise_manifest_version(version) for version in ordered]
        self.assertEqual(numbers[:4], ["1.00", "1.0001", "1.0012", "1.01"])
        self.assertEqual([float(number) for number in numbers], sorted(float(number) for number in numbers))
        self.assertEqual(len(set(map(float, numbers))), len(numbers))
        for unfit in ("1.100.0", "1.0.100", "1.0", "v1.0.0"):
            with self.subTest(unfit=unfit), self.assertRaises(ValueError):
                release.renoise_manifest_version(unfit)

    def test_the_reaper_sdks_notice_ships_with_the_extension(self):
        # Every bundle carries REAPER's extension and THIRD_PARTY_NOTICES.md (see the bundle's test): the notice names the
        # vendored SDK commit and its copyright, and gives its license word for word; the shim that repeats the SDK's
        # declaration names that license too.
        sdk = release.ROOT / release.REAPER_CRATE / "sdk"
        notices = (release.ROOT / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")
        section = notices.split("\n## REAPER extension SDK (Cockos)", 1)[1].split("\n## ", 1)[0]
        self.assertIn((sdk / "LICENSE").read_text(encoding="utf-8").strip(), section)
        self.assertIn(re.search(r"commit ([0-9a-f]{40})", (sdk / "SOURCE.txt").read_text(encoding="utf-8"))[1], section)
        self.assertIn("Copyright (C) 2006-2015, Cockos Incorporated", section)
        self.assertIn("Copyright (C) 2006-2015, Cockos Incorporated", (sdk / "reaper_plugin.h").read_text(encoding="utf-8"))
        self.assertIn("reaper-rs", section)
        # Each shim repeats an SDK declaration: each names its copyright and license, and the notice names each. A new
        # shim is added to this list, and so to the notice.
        shims = ("csurf.cpp", "preview.cpp")
        self.assertEqual(sorted(path.name for path in (release.ROOT / release.REAPER_CRATE / "shim").iterdir()), list(shims))
        for shim in shims:
            text = (release.ROOT / release.REAPER_CRATE / "shim" / shim).read_text(encoding="utf-8")
            self.assertIn("../sdk/LICENSE", text, shim)
            self.assertIn("Cockos", text, shim)
            self.assertIn(f"crates/reaper-kumi/shim/{shim}", section, shim)

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
