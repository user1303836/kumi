"""scripts/release.py's version, CHANGELOG, README and KUMI_CHANGES edits, on copies of this checkout's files.

Nothing here runs git or gh: those parts are the release itself.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_script", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release  # dataclasses look their module up there
SPEC.loader.exec_module(release)

FILES = ["package.json", "Cargo.lock", "CHANGELOG.md", "README.md", "README.ja.md", "README.zh-CN.md",
         "crates/kumi/Cargo.toml", "crates/kumi-common/Cargo.toml", "daws/own/daw-mcp-server/Cargo.toml",
         "crates/kumi-types/src/version.rs", "crates/kumi-types/Cargo.toml", "daws/ableton/ableton-mcp-server/Cargo.toml",
         "docs/en/KUMI_CHANGES.md", "docs/ja/KUMI_CHANGES.md", "docs/zh-CN/KUMI_CHANGES.md",
         release.REAPER_CARGO, release.RENOISE_VERSION, release.RENOISE_MANIFEST]
LONG = ("Kumi does a new thing, described at enough length that its changelog bullet wraps onto a second line, "
        "as most of them do.")


def current() -> tuple[str, str]:
    kumi = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]
    cargo = (ROOT / "daws/ableton/ableton-mcp-server/Cargo.toml").read_text(encoding="utf-8")
    return kumi, re.search(r'^version = "([^"]+)"', cargo, re.M)[1]


def daw_versions() -> tuple[str, str]:
    """REAPER's extension's and Renoise's tool's versions in this checkout."""
    reaper = re.search(r'^version = "([^"]+)"', (ROOT / release.REAPER_CARGO).read_text(encoding="utf-8"), re.M)[1]
    return reaper, re.search(r'return "([^"]+)"', (ROOT / release.RENOISE_VERSION).read_text(encoding="utf-8"))[1]


def daw_docs(kumi: str, reaper: str, renoise: str, later: dict[str, str] | None = None) -> dict[str, str]:
    """A doc in each language with REAPER's and Renoise's running ships-with lines, as the docs write them: the first
    entry, then each language's `later` entries (older releases), or a full stop when it's the first release of them."""
    later = later or {}
    lines = {"en": ("Each Kumi release ships with REAPER's extension: Kumi {kumi} with REAPER extension {reaper}{en}\n\n"
                    "And with Renoise's tool: Kumi {kumi} with Renoise tool {renoise}{en}\n"),
             "ja": ("Kumi のリリースには REAPER 拡張機能が付きます：Kumi {kumi} には REAPER 拡張機能 {reaper}{ja}\n\n"
                    "Renoise ツールも：Kumi {kumi} には Renoise ツール {renoise}{ja}\n"),
             "zh-CN": ("每个 Kumi 版本都附带 REAPER 扩展：Kumi {kumi} 附带 REAPER 扩展 {reaper}{zh}\n\n"
                       "以及 Renoise 工具：Kumi {kumi} 附带 Renoise 工具 {renoise}{zh}\n")}
    ends = {"en": later.get("en", "."), "ja": later.get("ja", "。"), "zh": later.get("zh-CN", "。")}
    return {f"docs/{language}/CONNECT_DAWS.md": text.format(kumi=kumi, reaper=reaper, renoise=renoise, **ends)
            for language, text in lines.items()}


def entry(changelog: str, version: str) -> str:
    return f"## {version} — " + changelog.split(f"\n## {version} — ", 1)[1].split("\n## ", 1)[0]


class Versions(unittest.TestCase):
    def test_the_next_version(self):
        self.assertEqual(release.bump("1.9.2"), "1.9.3")
        self.assertEqual(release.bump("1.9.9"), "1.9.10")
        self.assertEqual(release.bump("1.9.2", minor=True), "1.10.0")
        self.assertTrue(release.newer("1.9.10", "1.9.9"))
        self.assertFalse(release.newer("1.9.2", "1.9.2"))


class Moves(unittest.TestCase):
    def test_a_path_from_before_a_move_counts_where_it_is_now_and_is_read_where_it_was(self):
        with patch.object(release, "MOVES", [("crates/a", "daws/x/a"), ("daws/x/a/src/b.rs", "daws/x/a/src/b/mod.rs")]):
            self.assertEqual(release.now("crates/a/src/b.rs"), "daws/x/a/src/b/mod.rs")
            self.assertEqual(release.now("crates/a/Cargo.toml"), "daws/x/a/Cargo.toml")
            self.assertEqual(release.now("crates/ab/Cargo.toml"), "crates/ab/Cargo.toml")
            self.assertEqual(release.then("daws/x/a/src/b/mod.rs"), ["daws/x/a/src/b/mod.rs", "daws/x/a/src/b.rs", "crates/a/src/b.rs"])
            self.assertEqual(release.then("daws/x/a/Cargo.toml"), ["daws/x/a/Cargo.toml", "crates/a/Cargo.toml"])
            self.assertEqual(release.then("crates/kumi/Cargo.toml"), ["crates/kumi/Cargo.toml"])

    def test_this_checkouts_moves_lead_to_paths_it_has(self):
        # A lean worktree leaves out the oracles only Extended reads, which are the checkout's all the same.
        lean = {f"testdata/oracles/{line}" for line in (ROOT / "testdata/oracles/extended-only.txt").read_text(encoding="utf-8").splitlines()
                if line and not line.startswith("#")}
        for old, new in release.MOVES:
            here = release.now(new)
            self.assertTrue((ROOT / here).exists() or here in lean, f"{old} moved to {new}, now {here}, which isn't here")


class Sorting(unittest.TestCase):
    def test_what_live_loads_is_the_bridge(self):
        for path in ("daws/ableton/ableton-mcp-server/src/host/clip.rs", "crates/kumi-common/src/lib.rs",
                     "daws/ableton/remote-script/AbletonMcpBridge/__init__.py",
                     "daws/ableton/protocol/ableton-live-v1.operations.json", "daws/ableton/live-extension/src/extension.ts",
                     "vendor/willington/release.json"):
            self.assertTrue(release.ships_bridge(path), path)
        for path in ("daws/ableton/ableton-mcp-server/tests/it/host_clip.rs", "daws/ableton/remote-script/test_remote_script.py",
                     "daws/ableton/ableton-mcp-server/README.md", "crates/kumi-agent/src/lib.rs", ".github/workflows/ci.yml"):
            self.assertFalse(release.ships_bridge(path), path)

    def test_changelog_lines_and_where_they_go(self):
        host = ["daws/ableton/ableton-mcp-server/src/host/clip.rs", "daws/ableton/ableton-mcp-server/tests/it/host_clip.rs"]
        both = host + ["crates/kumi-agent/src/lib.rs"]
        self.assertEqual(release.classify(1, "Why.\n\nChangelog: A copy is refused.\n", host).lines,
                         [("bridge", "A copy is refused.")])
        self.assertEqual(release.classify(2, "Changelog: Kumi does X.", both).lines, [("kumi", "Kumi does X.")])
        self.assertEqual(release.classify(3, "Changelog (bridge): The bridge does Y.\nchangelog (Kumi): Kumi does Z.", both).lines,
                         [("bridge", "The bridge does Y."), ("kumi", "Kumi does Z.")])
        quiet = release.classify(4, "Changelog: none (CI and developer docs).", [".github/workflows/ci.yml"])
        self.assertEqual((quiet.lines, quiet.written), ([], True))
        self.assertFalse(release.classify(5, "Bumps a test dependency.", ["crates/kumi-agent/tests/support/package.json"]).written)
        # kumi-common is the bridge's too, but its lines stay Kumi's.
        self.assertEqual(release.classify(6, "Changelog: Shared thing.", ["crates/kumi-common/src/lib.rs"]).lines,
                         [("kumi", "Shared thing.")])

    def test_a_wrapped_changelog_line_is_flagged(self):
        wrapped = release.classify(7, "Changelog: Kumi does a thing that the author\r\nwrapped onto a second row.\n", [])
        self.assertEqual((wrapped.lines, wrapped.wrapped), ([("kumi", "Kumi does a thing that the author")], ["Kumi does a thing that the author"]))
        for body in ("Changelog: One row.\n\nMore about it.", "Changelog: One row.\n- a list item", "Changelog: One row.\n🤖 Generated"):
            self.assertEqual(release.classify(8, body, []).wrapped, [], body)


class BridgeGraph(unittest.TestCase):
    LOCK = """version = 4

[[package]]
name = "ableton-mcp-server"
version = "1.0.86"
dependencies = ["kumi-common", "serde 1.0.1"]

[[package]]
name = "kumi-common"
version = "1.9.4"
dependencies = ["itoa"]

[[package]]
name = "kumi-runtime"
version = "1.9.4"
dependencies = ["kumi-common", "tokio"]

[[package]]
name = "serde"
version = "1.0.1"

[[package]]
name = "itoa"
version = "1.0.15"

[[package]]
name = "tokio"
version = "1.47.0"
"""

    def test_what_the_host_reaches(self):
        self.assertEqual(release.bridge_graph(self.LOCK), {("serde", "1.0.1"), ("itoa", "1.0.15")})
        runtime_only = self.LOCK.replace('version = "1.47.0"', 'version = "1.48.0"')
        self.assertEqual(release.bridge_graph(runtime_only), release.bridge_graph(self.LOCK))
        host = self.LOCK.replace('version = "1.0.15"', 'version = "1.0.16"')
        self.assertEqual(release.bridge_graph(host) - release.bridge_graph(self.LOCK), {("itoa", "1.0.16")})

    def test_this_checkouts_lock(self):
        graph = release.bridge_graph((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
        self.assertTrue(graph)
        self.assertFalse({name for name, _ in graph} & {"ableton-mcp-server", "kumi-common", "kumi", "kumi-agent"})

    def test_reapers_extensions_graph(self):
        lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
        graph = release.bridge_graph(lock, "reaper-kumi", ("reaper-kumi", "daw-bridge"))
        self.assertIn("serde_json", {name for name, _ in graph})
        self.assertFalse({name for name, _ in graph} & {"reaper-kumi", "daw-bridge", "kumi", "kumi-agent", "ableton-mcp-server"})
        # A lock from before the extension: nothing to compare with.
        self.assertEqual(release.bridge_graph(self.LOCK, "reaper-kumi", ("reaper-kumi", "daw-bridge")), set())


class Edits(unittest.TestCase):
    def released(self, new_bridge: bool) -> tuple["release.Plan", dict[str, str]]:
        old, bridge = current()
        plan = release.Plan(old, release.bump(old), bridge, release.bump(bridge) if new_bridge else bridge, "2026-10-07",
                            [LONG], ["The bridge does a new thing."] if new_bridge else [])
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for name in FILES:
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, root / name)
            release.apply(root, plan)
            return plan, {name: (root / name).read_text(encoding="utf-8") for name in FILES}

    def test_a_release_with_the_same_bridge(self):
        plan, texts = self.released(new_bridge=False)
        self.assertEqual(json.loads(texts["package.json"])["version"], plan.new)
        self.assertIn(f'"{plan.new}"', texts["crates/kumi-types/src/version.rs"])
        for crate, folder in release.CRATES.items():
            self.assertIn(f'version = "{plan.new}"', texts[f"{folder}/Cargo.toml"])
            self.assertIn(f'name = "{crate}"\nversion = "{plan.new}"', texts["Cargo.lock"])
        self.assertIn(f'name = "ableton-mcp-server"\nversion = "{plan.bridge}"', texts["Cargo.lock"])
        self.assertIn(f'version = "{plan.bridge}"', texts["daws/ableton/ableton-mcp-server/Cargo.toml"])
        new = entry(texts["CHANGELOG.md"], plan.new)
        self.assertTrue(new.startswith(f"## {plan.new} — 2026-10-07\n\nShips with bridge {plan.bridge}, as {plan.old} did.\n\n- Kumi does"))
        self.assertIn("\n\nTested with Live", new)
        self.assertNotIn("### Bridge", new)
        self.assertTrue(all(len(line) <= release.WIDTH for line in new.splitlines()), new)
        self.assertLess(texts["CHANGELOG.md"].index(f"## {plan.new} — "), texts["CHANGELOG.md"].index(f"## {plan.old} — "))
        self.assertIn(f"- Kumi {plan.new} is currently supported", texts["README.md"])
        self.assertIn(f"- {plan.new} is tested with Live on macOS.", texts["README.md"])
        self.assertIn(f"Kumi {plan.new} は macOS", texts["README.ja.md"])
        self.assertIn(f"Kumi {plan.new} 已在 macOS", texts["README.zh-CN.md"])
        new, bridge = re.escape(plan.new), re.escape(plan.bridge)
        self.assertRegex(texts["docs/en/KUMI_CHANGES.md"], rf"Kumi [\d.]+ to {new} with bridge {bridge}, ")
        self.assertRegex(texts["docs/ja/KUMI_CHANGES.md"], rf"Kumi [\d.]+〜{new} にはブリッジ {bridge}、")
        self.assertRegex(texts["docs/zh-CN/KUMI_CHANGES.md"], rf"Kumi [\d.]+ 至 {new} 附带桥接 {bridge}，")

    def test_a_release_with_a_new_bridge(self):
        plan, texts = self.released(new_bridge=True)
        self.assertIn(f'name = "ableton-mcp-server"\nversion = "{plan.new_bridge}"', texts["Cargo.lock"])
        self.assertIn(f'version = "{plan.new_bridge}"', texts["daws/ableton/ableton-mcp-server/Cargo.toml"])
        new = entry(texts["CHANGELOG.md"], plan.new)
        self.assertIn(f"Ships with bridge {plan.new_bridge}, which Live loads when it restarts.", new)
        self.assertIn(f"\n\n### Bridge {plan.new_bridge}\n\n- The bridge does a new thing.", new)
        self.assertLess(new.index("- Kumi does"), new.index("Tested with Live"))
        self.assertLess(new.index("Tested with Live"), new.index("### Bridge"))
        self.assertTrue(all(len(line) <= release.WIDTH for line in new.splitlines()), new)
        new, nb, old_bridge = re.escape(plan.new), re.escape(plan.new_bridge), re.escape(plan.bridge)
        self.assertRegex(texts["docs/en/KUMI_CHANGES.md"], rf"Kumi {new} with bridge {nb}, [\d.]+(?: to [\d.]+)? with {old_bridge}, ")
        self.assertRegex(texts["docs/ja/KUMI_CHANGES.md"], rf"Kumi {new} にはブリッジ {nb}、[\d.]+(?:〜[\d.]+)? には {old_bridge}、")
        self.assertRegex(texts["docs/zh-CN/KUMI_CHANGES.md"], rf"Kumi {new} 附带桥接 {nb}，[\d.]+(?: 至 [\d.]+)? 附带 {old_bridge}，")

    def test_refuses_what_it_cant_follow(self):
        old, bridge = current()
        plan = release.Plan(old, release.bump(old), bridge, bridge, "2026-10-07", ["x"], [])
        with self.assertRaises(release.Refused):  # an Unreleased section would be lost
            release.changelog(f"# Changelog\n\n## Unreleased\n\n- y\n\n## {old} — 2026-10-06\n\nTested with Live.\n", plan)
        stale = release.Plan("0.0.1", "0.0.2", bridge, bridge, "2026-10-07", ["x"], [])
        with self.assertRaises(release.Refused):  # KUMI_CHANGES doesn't start with the release before
            release.ships_with((ROOT / "docs/en/KUMI_CHANGES.md").read_text(encoding="utf-8"), "en", stale)


class Notes(unittest.TestCase):
    def test_the_release_notes(self):
        tested = "Tested with Live 12.4 on macOS. On Windows, installing\nand updating are tested."
        same = release.Plan("1.9.2", "1.9.3", "1.0.85", "1.0.85", "2026-10-07", ["Kumi does X."], [])
        text = release.notes(same, tested, None)
        self.assertTrue(text.startswith("Kumi 1.9.3. Ships with bridge **1.0.85**, as 1.9.2 did, so Live needs no restart.\n"))
        self.assertIn("\n\n- Kumi does X.\n\n", text)
        self.assertNotIn("needs Live to restart", text)
        self.assertTrue(text.endswith("Tested with Live 12.4 on macOS. On Windows, installing and updating are tested.\n\n"
                                      + release.MARKER + "\n"))
        moved = release.Plan("1.9.2", "1.9.3", "1.0.85", "1.0.86", "2026-10-07", [], ["The bridge does Y."])
        text = release.notes(moved, tested, "Kumi 1.9.3 does Y.")
        self.assertTrue(text.startswith("Kumi 1.9.3 does Y. Ships with bridge **1.0.86**, which Live loads when it restarts.\n"))
        self.assertIn("\n\n**Bridge 1.0.86**\n- The bridge does Y.\n\n", text)
        self.assertIn("The new bridge needs Live to restart", text)


class DawSorting(unittest.TestCase):
    def test_what_reaper_and_renoise_load_is_their_bridges(self):
        for path in ("daws/reaper/reaper-kumi/src/lib.rs", "daws/reaper/reaper-kumi/build.rs", "daws/reaper/reaper-kumi/sdk/reaper_plugin.h",
                     "daws/reaper/reaper-kumi/Cargo.toml", "daws/own/daw-bridge/src/wire.rs"):
            self.assertTrue(release.ships_reaper(path), path)
        for path in ("daws/reaper/reaper-kumi/tests/it/loads.rs", "daws/reaper/reaper-kumi/src/changes/tests.rs",
                     "daws/reaper/reaper-kumi/examples/accept_reaper.rs", "daws/reaper/reaper-kumi/sdk/SOURCE.md",
                     "daws/own/daw-bridge/tests/it/vectors.rs", "daws/own/daw-mcp-server/src/host.rs", "daws/renoise/tests/test_changes.lua"):
            self.assertFalse(release.ships_reaper(path), path)
        # daw-bridge's parts the extension doesn't link are Kumi's alone: a fix there is no new extension.
        for path in ("daws/own/daw-bridge/src/lifecycle.rs", "daws/own/daw-bridge/src/detect.rs", "daws/own/daw-bridge/src/doctor.rs",
                     "daws/own/daw-bridge/src/install.rs", "daws/own/daw-bridge/src/probe.rs"):
            self.assertFalse(release.ships_reaper(path), path)
        for path in ("daws/own/daw-bridge/src/handshake.rs", "daws/own/daw-bridge/src/owner.rs", "daws/own/daw-bridge/Cargo.toml"):
            self.assertTrue(release.ships_reaper(path), path)
        for path in ("daws/renoise/com.kumi.Bridge.xrnx/payload/changes.lua", "daws/renoise/com.kumi.Bridge.xrnx/manifest.xml",
                     "daws/renoise/com.kumi.Bridge.xrnx/main.lua"):
            self.assertTrue(release.ships_renoise(path), path)
        for path in ("daws/renoise/tests/test_changes.lua", "daws/renoise/tests/mock_song.lua", "daws/reaper/reaper-kumi/src/lib.rs"):
            self.assertFalse(release.ships_renoise(path), path)
        # Neither is Live's bridge.
        self.assertFalse(release.ships_bridge("daws/reaper/reaper-kumi/src/lib.rs") or release.ships_bridge("daws/renoise/com.kumi.Bridge.xrnx/main.lua"))

    def test_changelog_lines_go_to_the_daw_whose_bridge_alone_changed(self):
        reaper, renoise = ["daws/reaper/reaper-kumi/src/changes/items.rs"], ["daws/renoise/com.kumi.Bridge.xrnx/payload/changes.lua"]
        line = "Changelog: A new thing."
        self.assertEqual(release.classify(1, line, reaper + ["daws/reaper/reaper-kumi/src/changes/tests.rs", "docs/en/X.md"]).lines,
                         [("reaper", "A new thing.")])
        self.assertEqual(release.classify(2, line, renoise + ["daws/renoise/tests/test_changes.lua"]).lines, [("renoise", "A new thing.")])
        for paths in (reaper + renoise, reaper + ["daws/own/kumi-daw-own/src/provider.rs"], renoise + ["daws/own/daw-mcp-server/src/tools.rs"],
                      ["daws/own/daw-bridge/src/wire.rs"]):  # daw-bridge is Kumi's too
            with self.subTest(paths=paths):
                self.assertEqual(release.classify(3, line, paths).lines, [("kumi", "A new thing.")])
        # Live's bridge's rule is as it was: its own changes with REAPER's put the lines under Live's bridge.
        self.assertEqual(release.classify(4, line, reaper + ["daws/ableton/ableton-mcp-server/src/host/clip.rs"]).lines,
                         [("bridge", "A new thing.")])
        tagged = release.classify(5, "Changelog (reaper): R.\nChangelog (Renoise): N.\nChangelog: K.", ["crates/kumi/src/lib.rs"])
        self.assertEqual(tagged.lines, [("reaper", "R."), ("renoise", "N."), ("kumi", "K.")])


class DawPlans(unittest.TestCase):
    def plan(self, changes, tag_has=True, head_has=True, **flags):
        """plan_release for these pull requests ((body, paths) each), main at 1.0.93 / 0.1.0 / 1.0.0."""
        args = argparse.Namespace(version=None, minor=False, bridge=False, no_bridge=False, reaper=False, no_reaper=False,
                                  renoise=False, no_renoise=False, date="2026-10-20")
        for name, value in flags.items():
            setattr(args, name, value)
        changes = [release.classify(number, body, paths) for number, (body, paths) in enumerate(changes, 1)]
        def versions_at(ref):
            has = head_has if ref == "HEAD" else tag_has
            return ("1.0.93", "0.1.0", "1.0.0") if has else ("1.0.93", None, None)
        with patch.object(release, "versions_at", side_effect=versions_at):
            return release.plan_release(args, "HEAD", "v1.19.3", changes)

    REAPER = ("Changelog: The extension does Y.", ["daws/reaper/reaper-kumi/src/changes/items.rs"])
    RENOISE = ("Changelog: The tool does Z.", ["daws/renoise/com.kumi.Bridge.xrnx/payload/changes.lua"])
    LIVE = ("Changelog: The bridge does X.", ["daws/ableton/ableton-mcp-server/src/host/clip.rs"])
    KUMI = ("Changelog: Kumi does W.", ["crates/kumi/src/lib.rs"])

    def test_a_daw_bridge_gets_a_new_version_when_what_it_loads_changed(self):
        plan = self.plan([self.KUMI, self.REAPER])
        self.assertEqual((plan.new_reaper, plan.new_renoise, plan.new_bridge), ("0.1.1", "1.0.0", "1.0.93"))
        self.assertEqual((plan.kumi, plan.reaper_lines, plan.renoise_lines), (["Kumi does W."], ["The extension does Y."], []))
        self.assertEqual((plan.moved(), plan.brought, plan.olds()), (["reaper"], [], ["1.19.3", "0.1.0"]))
        plan = self.plan([self.RENOISE, self.REAPER, self.LIVE])
        self.assertEqual((plan.new_reaper, plan.new_renoise, plan.new_bridge), ("0.1.1", "1.0.1", "1.0.94"))
        self.assertEqual(plan.olds(), ["1.19.3", "1.0.93", "0.1.0", "1.0.0"])

    def test_each_daws_flags_and_live_bridges_are_its_own(self):
        kept = self.plan([self.KUMI, self.REAPER], no_reaper=True)
        self.assertEqual((kept.new_reaper, kept.moved()), ("0.1.0", []))
        self.assertEqual(kept.kumi, ["Kumi does W.", "The extension does Y."])  # its lines are Kumi's then
        forced = self.plan([self.KUMI], renoise=True)
        self.assertEqual((forced.new_renoise, forced.new_reaper), ("1.0.1", "0.1.0"))
        # --no-bridge is Live's bridge's alone, and --bridge too.
        self.assertEqual((lambda p: (p.new_bridge, p.new_reaper))(self.plan([self.LIVE, self.REAPER], no_bridge=True)), ("1.0.93", "0.1.1"))
        self.assertEqual((lambda p: (p.new_bridge, p.new_reaper))(self.plan([self.KUMI], bridge=True)), ("1.0.94", "0.1.0"))

    def test_a_daw_bridge_the_last_release_hadnt_ships_as_it_is(self):
        plan = self.plan([self.KUMI, self.REAPER, self.RENOISE], tag_has=False)
        self.assertEqual((plan.new_reaper, plan.new_renoise, plan.brought, plan.moved()), ("0.1.0", "1.0.0", ["reaper", "renoise"], []))
        self.assertEqual(plan.kumi, ["Kumi does W.", "The extension does Y.", "The tool does Z."])
        # Given a version all the same, it's still brought, not updated: no `kumi update` puts in what wasn't there.
        forced = self.plan([self.KUMI], tag_has=False, reaper=True)
        self.assertEqual((forced.new_reaper, forced.brought, forced.moved()), ("0.1.1", ["reaper", "renoise"], []))
        self.assertEqual(release.daws_said(forced), "Brings REAPER extension 0.1.1 and Renoise tool 1.0.0: `kumi --daw reaper bridge` "
                                                    "and `kumi --daw renoise bridge` put them in.")
        # A main without them says nothing of them, and refuses to give them a version.
        plan = self.plan([self.KUMI, self.REAPER], tag_has=False, head_has=False)
        self.assertEqual((plan.reaper, plan.new_reaper, plan.brought), (None, None, []))
        with self.assertRaisesRegex(release.Refused, "--reaper: main at HEAD has no REAPER extension"):
            self.plan([self.KUMI], tag_has=False, head_has=False, reaper=True)

    def test_the_flags_pair_up_on_the_command_line(self):
        seen = []
        with patch.object(release, "release", side_effect=seen.append), patch.object(sys, "argv", ["release.py", "--no-bridge", "--reaper"]):
            release.main()
        self.assertEqual((seen[0].no_bridge, seen[0].reaper, seen[0].no_reaper, seen[0].renoise), (True, True, False, False))
        for pair in (["--reaper", "--no-reaper"], ["--renoise", "--no-renoise"]):
            with self.subTest(pair=pair), patch.object(sys, "argv", ["release.py", *pair]), patch("sys.stderr"):
                with self.assertRaises(SystemExit):
                    release.main()


class DawEdits(unittest.TestCase):
    def released(self, reaper=False, renoise=False, docs=None, brought=(), new_bridge=False):
        """apply() on copies of this checkout's files (and `docs`): the plan, every file's text after, the files apply
        said it changed, and the ones whose bytes did change."""
        old, bridge = current()
        reaper_now, renoise_now = daw_versions()
        plan = release.Plan(old, release.bump(old), bridge, release.bump(bridge) if new_bridge else bridge, "2026-10-20", [LONG],
                            ["The bridge does a new thing."] if new_bridge else [],
                            reaper_now, release.bump(reaper_now) if reaper else reaper_now,
                            renoise_now, release.bump(renoise_now) if renoise else renoise_now,
                            ["The extension does a new thing."] if reaper else [], ["The tool does a new thing."] if renoise else [],
                            list(brought))
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for name in FILES:
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, root / name)
            for name, text in (docs or {}).items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text(text, encoding="utf-8")
            names = sorted(path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file())
            before = {name: (root / name).read_bytes() for name in names}
            changed = release.apply(root, plan)
            texts = {name: (root / name).read_text(encoding="utf-8") for name in names}
            really = sorted(name for name in names if (root / name).read_bytes() != before[name])
            return plan, texts, changed, really

    def test_new_versions_of_both_daw_bridges(self):
        plan, texts, changed, really = self.released(reaper=True, renoise=True)
        self.assertEqual(sorted(changed), really)
        self.assertIn(f'name = "reaper-kumi"\nversion = "{plan.new_reaper}"', texts["Cargo.lock"])
        self.assertIn(f'version = "{plan.new_reaper}"', texts[release.REAPER_CARGO])
        self.assertIn(f'return "{plan.new_renoise}"', texts[release.RENOISE_VERSION])
        self.assertIn(f"<Version>{release.native.renoise_manifest_version(plan.new_renoise)}</Version>", texts[release.RENOISE_MANIFEST])
        self.assertTrue({release.REAPER_CARGO, release.RENOISE_VERSION, release.RENOISE_MANIFEST} <= set(changed))
        new = entry(texts["CHANGELOG.md"], plan.new)
        self.assertIn(f"Ships with REAPER extension {plan.new_reaper} and Renoise tool {plan.new_renoise}, which REAPER and Renoise "
                      "load when they restart; `kumi update` puts them in.", " ".join(new.split()))
        self.assertTrue(all(len(line) <= release.WIDTH for line in new.splitlines()), new)
        self.assertIn(f"\n\n### REAPER extension {plan.new_reaper}\n\n- The extension does a new thing.", new)
        self.assertIn(f"\n\n### Renoise tool {plan.new_renoise}\n\n- The tool does a new thing.", new)
        self.assertLess(new.index("Tested with"), new.index("### REAPER extension"))
        self.assertLess(new.index("### REAPER extension"), new.index("### Renoise tool"))

    def test_a_new_version_of_one_daw_bridge(self):
        plan, texts, changed, really = self.released(reaper=True, new_bridge=True)
        self.assertEqual(sorted(changed), really)
        self.assertNotIn(release.RENOISE_VERSION, changed)
        self.assertNotIn(release.RENOISE_MANIFEST, changed)
        new = entry(texts["CHANGELOG.md"], plan.new)
        flat = " ".join(new.split())
        self.assertIn(f"with Live closed. Ships with REAPER extension {plan.new_reaper}, which REAPER loads when it restarts; "
                      "`kumi update` puts it in.", flat)
        self.assertNotIn("Renoise", new)
        self.assertLess(new.index(f"### Bridge {plan.new_bridge}"), new.index(f"### REAPER extension {plan.new_reaper}"))

    def test_daw_bridges_as_they_were_say_nothing(self):
        plan, texts, changed, really = self.released()
        self.assertEqual(sorted(changed), really)
        # what a release changed before REAPER and Renoise, the host for them and kumi-types, less kumi-runtime's manifest
        # (kumi-agent, which took its place, isn't at Kumi's version)
        self.assertEqual(len(changed), 14)
        new = entry(texts["CHANGELOG.md"], plan.new)
        self.assertNotIn("REAPER", new)
        self.assertNotIn("Renoise", new)

    def test_the_notes_say_the_new_daw_bridges_and_their_lines(self):
        reaper_now = daw_versions()[0]
        both = release.Plan("1.20.0", "1.20.1", "1.0.93", "1.0.93", "2026-10-20", ["Kumi does X."], [], reaper_now, "9.9.9", "1.0.0", "1.0.1",
                            ["The extension does Y."], [])
        text = release.notes(both, "Tested with Live.", None)
        self.assertTrue(text.startswith("Kumi 1.20.1. Ships with bridge **1.0.93**, as 1.20.0 did, so Live needs no restart. Ships with "
                                        "REAPER extension **9.9.9** and Renoise tool **1.0.1**, which REAPER and Renoise load when they "
                                        "restart; `kumi update` puts them in.\n"), text)
        self.assertIn("\n\n**REAPER extension 9.9.9**\n- The extension does Y.\n\n", text)
        self.assertNotIn("**Renoise tool", text)  # no line of its own: no heading
        same = release.Plan("1.20.0", "1.20.1", "1.0.93", "1.0.93", "2026-10-20", ["Kumi does X."], [], "0.1.0", "0.1.0", "1.0.0", "1.0.0")
        self.assertNotIn("REAPER", release.notes(same, "Tested with Live.", None))
        brought = release.Plan("1.19.3", "1.20.0", "1.0.93", "1.0.93", "2026-10-20", ["Kumi does X."], [], "0.1.0", "0.1.0", "1.0.0", "1.0.0",
                               brought=["reaper", "renoise"])
        self.assertIn("no restart. Brings REAPER extension **0.1.0** and Renoise tool **1.0.0**: `kumi --daw reaper bridge` and "
                      "`kumi --daw renoise bridge` put them in.\n", release.notes(brought, "Tested with Live.", None))
        body = release.pr_body(both, [release.classify(7, "Changelog: The extension does Y.", ["daws/reaper/reaper-kumi/src/lib.rs"])], "a" * 40, [])
        self.assertIn(f"- **REAPER extension 9.9.9**\n  - #7: The extension does Y.\n- **Renoise tool 1.0.1**\n  - no line of its own", body)
        self.assertIn(f"- REAPER extension {reaper_now} → 9.9.9 in `daws/reaper/reaper-kumi/Cargo.toml` and Cargo.lock.", body)
        self.assertIn(f'- "1.20.0" or "{reaper_now}" or "1.0.0" left outside', body)
        # A bridge kept at its version: its lines are listed under Kumi's.
        renoise_only = release.classify(8, "Changelog: The tool does Z.", ["daws/renoise/com.kumi.Bridge.xrnx/payload/changes.lua"])
        body = release.pr_body(same, [renoise_only], "a" * 40, [])
        self.assertIn("### What ships\n- **Kumi**\n  - #8: The tool does Z.\n", body)
        self.assertNotIn("**Renoise tool", body)

    def test_the_docs_daw_lines_move_on_with_each_release(self):
        old, _ = current()
        reaper, renoise = daw_versions()
        new = release.bump(old)
        # The first release of each: a full stop after its only entry. REAPER's gets a new version, Renoise's doesn't.
        plan, texts, changed, really = self.released(reaper=True, docs=daw_docs(old, reaper, renoise))
        self.assertEqual(sorted(changed), really)
        new_reaper = plan.new_reaper
        self.assertIn(f"Kumi {new} with REAPER extension {new_reaper}, {old} with {reaper}.\n", texts["docs/en/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {old} to {new} with Renoise tool {renoise}.\n", texts["docs/en/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {new} には REAPER 拡張機能 {new_reaper}、{old} には {reaper}。\n", texts["docs/ja/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {old}〜{new} には Renoise ツール {renoise}。\n", texts["docs/ja/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {new} 附带 REAPER 扩展 {new_reaper}，{old} 附带 {reaper}。\n", texts["docs/zh-CN/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {old} 至 {new} 附带 Renoise 工具 {renoise}。\n", texts["docs/zh-CN/CONNECT_DAWS.md"])
        self.assertTrue({f"docs/{language}/CONNECT_DAWS.md" for language in ("en", "ja", "zh-CN")} <= set(changed))
        # A line with older releases after its first entry, whose range runs to the last release.
        later = {"en": ", 1.19.0 to 1.19.1 with 0.0.9.", "ja": "、1.19.0〜1.19.1 には 0.0.9。", "zh-CN": "，1.19.0 至 1.19.1 附带 0.0.9。"}
        docs = daw_docs(f"1.19.2 to {old}", reaper, renoise, later)
        docs = {name: text.replace(f"1.19.2 to {old} には", f"1.19.2〜{old} には").replace(f"1.19.2 to {old} 附带", f"1.19.2 至 {old} 附带")
                for name, text in docs.items()}
        plan, texts, _, _ = self.released(renoise=True, docs=docs)
        self.assertIn(f"Kumi 1.19.2 to {new} with REAPER extension {reaper}, 1.19.0 to 1.19.1 with 0.0.9.", texts["docs/en/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {new} with Renoise tool {plan.new_renoise}, 1.19.2 to {old} with {renoise}, 1.19.0 to 1.19.1 with 0.0.9.",
                      texts["docs/en/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi {new} には Renoise ツール {plan.new_renoise}、1.19.2〜{old} には {renoise}、1.19.0〜1.19.1 には 0.0.9。",
                      texts["docs/ja/CONNECT_DAWS.md"])
        self.assertIn(f"Kumi 1.19.2 至 {new} 附带 REAPER 扩展 {reaper}，1.19.0 至 1.19.1 附带 0.0.9。", texts["docs/zh-CN/CONNECT_DAWS.md"])

    def test_docs_without_daw_lines_are_left_alone_and_partial_or_stale_ones_refused(self):
        old, _ = current()
        reaper, renoise = daw_versions()
        _, _, changed, really = self.released(reaper=True, renoise=True)
        self.assertEqual(sorted(changed), really)
        self.assertFalse([name for name in changed if "CONNECT" in name])
        docs = daw_docs(old, reaper, renoise)
        for broken, said in [({"docs/ja/CONNECT_DAWS.md": "", "docs/zh-CN/CONNECT_DAWS.md": ""}, "is in docs/en but not in docs/ja, docs/zh-CN"),
                             ({"docs/en/MORE.md": docs["docs/en/CONNECT_DAWS.md"]}, "docs/en: expected one REAPER extension ships-with line, found 2"),
                             ({name: text.replace(f"Kumi {old} ", "Kumi 1.0.0 ") for name, text in docs.items()},
                              f"line starts 'Kumi 1.0.0 with REAPER extension {reaper}.', not Kumi {old} with REAPER extension {reaper}")]:
            with self.subTest(said=said), self.assertRaisesRegex(release.Refused, re.escape(said)):
                self.released(reaper=True, docs={**docs, **broken})

    def test_a_daw_bridge_the_release_brings_keeps_its_line_as_written(self):
        old, _ = current()
        reaper, renoise = daw_versions()
        new = release.bump(old)
        docs = daw_docs(new, reaper, renoise)
        _, texts, changed, really = self.released(docs=docs, brought=("reaper", "renoise"))
        self.assertEqual(sorted(changed), really)
        self.assertEqual(texts["docs/en/CONNECT_DAWS.md"], docs["docs/en/CONNECT_DAWS.md"])
        self.assertFalse([name for name in changed if "CONNECT" in name])
        with self.assertRaisesRegex(release.Refused, re.escape(f"the first release of it says the release that brings it, Kumi {new} with REAPER extension {reaper}")):
            self.released(docs=daw_docs(old, reaper, renoise), brought=("reaper", "renoise"))

    def test_this_checkouts_docs_have_each_daw_line_in_every_language_or_none(self):
        versions = dict(zip(release.DAWS, daw_versions()))
        for daw in release.DAWS:
            for language, path in release.daw_lines(ROOT, daw).items():
                entry_found = re.search(release.DAW_SHIPS[daw][language][0], path.read_text(encoding="utf-8"))
                self.assertEqual(entry_found["bridge"], versions[daw], f"{path}: the {daw} line's first entry")


if __name__ == "__main__":
    unittest.main()
