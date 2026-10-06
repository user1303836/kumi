"""scripts/release.py's version, CHANGELOG, README and KUMI_CHANGES edits, on copies of this checkout's files.

Nothing here runs git or gh: those parts are the release itself.
"""
import importlib.util
import json
from pathlib import Path
import re
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_script", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release  # dataclasses look their module up there
SPEC.loader.exec_module(release)

FILES = ["package.json", "Cargo.lock", "CHANGELOG.md", "README.md", "README.ja.md", "README.zh-CN.md",
         "crates/kumi/Cargo.toml", "crates/kumi-common/Cargo.toml", "crates/kumi-runtime/Cargo.toml",
         "crates/kumi-runtime/src/version.rs", "crates/ableton-mcp-server/Cargo.toml",
         "docs/en/KUMI_CHANGES.md", "docs/ja/KUMI_CHANGES.md", "docs/zh-CN/KUMI_CHANGES.md"]
LONG = ("Kumi does a new thing, described at enough length that its changelog bullet wraps onto a second line, "
        "as most of them do.")


def current() -> tuple[str, str]:
    kumi = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]
    cargo = (ROOT / "crates/ableton-mcp-server/Cargo.toml").read_text(encoding="utf-8")
    return kumi, re.search(r'^version = "([^"]+)"', cargo, re.M)[1]


def entry(changelog: str, version: str) -> str:
    return f"## {version} — " + changelog.split(f"\n## {version} — ", 1)[1].split("\n## ", 1)[0]


class Versions(unittest.TestCase):
    def test_the_next_version(self):
        self.assertEqual(release.bump("1.9.2"), "1.9.3")
        self.assertEqual(release.bump("1.9.9"), "1.9.10")
        self.assertEqual(release.bump("1.9.2", minor=True), "1.10.0")
        self.assertTrue(release.newer("1.9.10", "1.9.9"))
        self.assertFalse(release.newer("1.9.2", "1.9.2"))


class Sorting(unittest.TestCase):
    def test_what_live_loads_is_the_bridge(self):
        for path in ("crates/ableton-mcp-server/src/host/clip.rs", "remote-script/AbletonMcpBridge/__init__.py",
                     "protocol/ableton-live-v1.operations.json", "apps/live-extension/src/extension.ts",
                     "vendor/willington/release.json"):
            self.assertTrue(release.ships_bridge(path), path)
        for path in ("crates/ableton-mcp-server/tests/host_clip.rs", "remote-script/test_remote_script.py",
                     "crates/ableton-mcp-server/README.md", "crates/kumi-runtime/src/agent.rs", ".github/workflows/ci.yml"):
            self.assertFalse(release.ships_bridge(path), path)

    def test_changelog_lines_and_where_they_go(self):
        host = ["crates/ableton-mcp-server/src/host/clip.rs", "crates/ableton-mcp-server/tests/host_clip.rs"]
        both = host + ["crates/kumi-runtime/src/agent.rs"]
        self.assertEqual(release.classify(1, "Why.\n\nChangelog: A copy is refused.\n", host).lines,
                         [("bridge", "A copy is refused.")])
        self.assertEqual(release.classify(2, "Changelog: Kumi does X.", both).lines, [("kumi", "Kumi does X.")])
        self.assertEqual(release.classify(3, "Changelog (bridge): The bridge does Y.\nchangelog (Kumi): Kumi does Z.", both).lines,
                         [("bridge", "The bridge does Y."), ("kumi", "Kumi does Z.")])
        quiet = release.classify(4, "Changelog: none (CI and developer docs).", [".github/workflows/ci.yml"])
        self.assertEqual((quiet.lines, quiet.written), ([], True))
        self.assertFalse(release.classify(5, "Bumps a test dependency.", ["crates/kumi-runtime/tests/support/package.json"]).written)


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
        self.assertIn(f'"{plan.new}"', texts["crates/kumi-runtime/src/version.rs"])
        for crate in release.CRATES:
            self.assertIn(f'version = "{plan.new}"', texts[f"crates/{crate}/Cargo.toml"])
            self.assertIn(f'name = "{crate}"\nversion = "{plan.new}"', texts["Cargo.lock"])
        self.assertIn(f'name = "ableton-mcp-server"\nversion = "{plan.bridge}"', texts["Cargo.lock"])
        self.assertIn(f'version = "{plan.bridge}"', texts["crates/ableton-mcp-server/Cargo.toml"])
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
        self.assertIn(f'version = "{plan.new_bridge}"', texts["crates/ableton-mcp-server/Cargo.toml"])
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
        self.assertTrue(text.endswith("Tested with Live 12.4 on macOS. On Windows, installing and updating are tested.\n"))
        moved = release.Plan("1.9.2", "1.9.3", "1.0.85", "1.0.86", "2026-10-07", [], ["The bridge does Y."])
        text = release.notes(moved, tested, "Kumi 1.9.3 does Y.")
        self.assertTrue(text.startswith("Kumi 1.9.3 does Y. Ships with bridge **1.0.86**, which Live loads when it restarts.\n"))
        self.assertIn("\n\n**Bridge 1.0.86**\n- The bridge does Y.\n\n", text)
        self.assertIn("The new bridge needs Live to restart", text)


if __name__ == "__main__":
    unittest.main()
