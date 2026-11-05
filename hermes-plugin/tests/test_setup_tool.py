"""kumi_setup and `hermes kumi setup` (C8) through the plugin, against the fixture runtime's setup_daw: what the model
can plan, what the producer is asked, that only the plan the producer approved runs (CC-5), and the terminal command."""
from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import re
import tarfile
import tempfile
import unittest
from unittest import mock

import support

kumi = support.plugin()
commands = support.module("commands")
policy = support.module("policy")
runtime = support.module("runtime")
setup = support.module("setup")


class Tty(io.StringIO):
    def isatty(self) -> bool:
        return True


def setup_daw_calls(data) -> list[dict]:
    """The setup_daw calls the fixture runtime received, without Hermes's `_host`."""
    return [{key: value for key, value in message["params"]["arguments"].items() if key != "_host"}
            for message in support.requests(data / "state") if message.get("params", {}).get("name") == "setup_daw"]


def desktop_host(test, *args, **kwargs):
    """`support.host`, for a chat in Hermes Desktop, which shows setup's approval whole (the surface tests below cover
    the others)."""
    found = support.host(test, *args, **kwargs)
    found[1].surface = "desktop"
    return found


def setUpModule() -> None:
    """No ffmpeg on the test machine reaches a plan: setup searches only the folders a test gives it."""
    searching = mock.patch.object(setup, "_ffmpeg_search", return_value=[])
    searching.start()
    unittest.addModuleCleanup(searching.stop)


class Surface(unittest.TestCase):
    """Setup asks only where Hermes shows every line of its plan (rgzm, #954 LOW 17): never cut, never summarized."""

    def test_desktop_asks_with_the_whole_plan_and_a_terminal_chat_sends_the_producer_to_hermes_kumi_setup(self):
        host, ctx, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        real = setup.approval

        def long_plan(*args, **kwargs):  # longer than the terminal's panel on any machine (temp paths differ)
            shown = real(*args, **kwargs)
            return {**shown, "effects": [*shown["effects"], *(f"Writes: /Kumi/part {n}" for n in range(10))]}

        with mock.patch.object(setup, "approval", side_effect=long_plan):
            shown = long_plan("ableton", data_dir=data)
            asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1", tool_call_id="c1")
            self.assertEqual((asked["action"], asked["message"].splitlines()),
                             ("approve", [shown["title"], *(f"• {line}" for line in shown["effects"])]))
            for surface in ("cli", "tui"):
                with self.subTest(surface):
                    ctx.surface = surface
                    refused = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1",
                                                   tool_call_id=f"c-{surface}")
                    self.assertEqual(refused["action"], "block")
                    self.assertIn("Use `hermes kumi setup` in a terminal", refused["message"])
                    self.assertNotIn("Kumi Ears.amxd", refused["message"], "it doesn't ask with part of the plan")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, setup_daw_calls(data), "nothing was installed")

    def test_a_plan_that_fits_the_terminal_panel_is_asked_there_whole(self):
        host, ctx, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        real = setup.approval

        def short_plan(*args, **kwargs):  # shorter than the panel on any machine
            return {**real(*args, **kwargs), "title": "Kumi: install Kumi's part in Live", "effects": ["Writes: /Kumi/part"]}

        ctx.surface = "tui"
        with mock.patch.object(setup, "approval", side_effect=short_plan):
            asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1", tool_call_id="c1")
            self.assertEqual((asked["action"], asked["message"]),
                             ("approve", "Kumi: install Kumi's part in Live\n• Writes: /Kumi/part"))
            ctx.surface = RuntimeError("the plugin host's channel is gone")
            with self.assertLogs(level="WARNING"):
                unknown = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1",
                                               tool_call_id="c2")
        self.assertEqual(unknown["action"], "block", "where Hermes can't say, nothing is taken as shown whole")
        self.assertNotIn("Writes:", unknown["message"], "it doesn't ask with part of the plan")
        self.assertFalse(policy.shows_whole("x", "unknown"))

    def test_a_plan_shown_whole_is_asked_on_every_surface_and_a_chat_app_that_would_cut_it_refuses(self):
        short = "Kumi: set up Kumi 1.24.67's runtime\n• Download Kumi 1.24.67 (26.0 MB)"
        for surface in ("cli", "tui", "desktop", "telegram", "discord", "slack", "acp", ""):
            self.assertTrue(policy.shows_whole(short, surface), surface)
        rows = "\n".join(f"• line {n} of a plan that says what it writes" for n in range(12))
        self.assertFalse(policy.shows_whole(rows, "cli"), "more than 10 rows of 64 columns")
        self.assertTrue(policy.shows_whole(rows, "desktop"))
        wide = "• Writes: /" + "a" * 70
        self.assertTrue(policy.shows_whole(wide, "tui"), "a long path wraps onto the next row, still in view")
        self.assertFalse(policy.shows_whole("• Writes: /" + "a" * 700, "tui"), "one path longer than 10 rows isn't")
        self.assertTrue(policy.shows_whole("x" * 300, "discord"))
        self.assertFalse(policy.shows_whole("x" * 301, "discord"))
        self.assertTrue(policy.shows_whole("x" * 500, "slack"))
        self.assertFalse(policy.shows_whole("." * 251, "telegram"), "Telegram counts its escapes")


class SetupTool(unittest.TestCase):
    def test_planning_the_runtime_names_the_download_and_starts_nothing(self):
        host, _, data = desktop_host(self, installed=False)
        self.assertIsNone(policy.pre_tool_call(host, "kumi_setup", {"action": "plan"}), "a plan needs no approval")
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("runtime", "kumi_setup install"))
        self.assertIn("https://github.com/user1303836/kumi/releases/download/",
                      setup.approval("ableton", data_dir=data)["effects"][0], "the producer's approval names it")
        self.assertIsNone(host.runtimes.get(data), "planning the runtime step starts no runtime")

    def test_the_daw_step_is_planned_from_the_runtimes_dry_run_with_its_exact_paths(self):
        host, _, data = desktop_host(self, daw_component=False)
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("daw_component", "kumi_setup install"))
        paths = json.loads((data / "setup" / "plan.json").read_text(encoding="utf-8"))["paths"]  # as recorded
        self.assertTrue(any(path.endswith("Kumi Ears.amxd") for path in paths), paths)
        self.assertEqual(setup_daw_calls(data), [{"daw": "ableton", "dry_run": True}])
        asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1", tool_call_id="c1")
        self.assertEqual(asked["action"], "approve")
        self.assertIn("Kumi Ears.amxd", asked["message"])
        shown = setup.approval("ableton", data_dir=data)
        self.assertEqual(asked["message"].splitlines(), [shown["title"], *(f"• {line}" for line in shown["effects"])],
                         "setup's lines verbatim: nothing cut or collapsed")
        for path in paths:
            self.assertIn(f"• Writes: {path}", asked["message"].splitlines(), "each path it writes, on its own line")
        self.assertEqual(asked["rule_key"], f"kumi:setup:{plan['digest']}", "'always' covers this exact plan only")

    def test_install_or_adopt_without_a_plan_is_blocked_before_anyone_is_asked(self):
        host, _, _ = desktop_host(self, installed=False)
        for action in ("install", "adopt"):
            with self.subTest(action):
                decision = policy.pre_tool_call(host, "kumi_setup", {"action": action}, tool_call_id="c1")
                self.assertEqual(decision, {"action": "block",
                                            "message": "Kumi didn't set anything up: run kumi_setup plan first."})
        support.call(host, "kumi_setup", {"action": "plan"})
        self.assertEqual(policy.pre_tool_call(host, "kumi_setup", {"action": "adopt"})["action"], "block",
                         "a runtime plan is never approved as an adoption")

    def test_a_runtime_working_in_another_daw_is_never_asked_to_set_up_this_one(self):
        host, _, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_connect", {"daw": "reaper"}, session_id="s1")
        planned = support.call(host, "kumi_setup", {"action": "plan", "daw": "ableton"}, session_id="s2")
        self.assertEqual(planned["error"]["code"], "other_daw")
        self.assertEqual(setup_daw_calls(data), [])

    def test_an_approved_install_carries_out_the_daw_step_it_showed(self):
        host, _, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        installed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertEqual(setup_daw_calls(data)[-1], {"daw": "ableton", "dry_run": False})
        parts = setup.status(daw="ableton", data_dir=data, plugin_dir=host.plugin_dir)["daw_component"]["parts"]
        self.assertEqual([(part["name"], part["state"]) for part in parts], [("Remote Script", "same"),
                                                                             ("Kumi Ears", "same")])

    def test_a_denied_install_or_one_that_skipped_the_hook_changes_nothing(self):
        host, _, data = desktop_host(self, daw_component=False)
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertIn("blocked", support.call(host, "kumi_setup", {"action": "install"}, session_id="s1", answer="deny"))
        skipped = support.handle(host, "kumi_setup", {"action": "install", "expected_digest": plan["digest"]},
                                 session_id="s1")
        self.assertEqual(skipped["error"]["code"], "not_approved", "a digest from the model is never used")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, setup_daw_calls(data))

    def test_a_plan_another_chat_made_after_the_approval_is_not_installed(self):
        host, _, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        hook = {"session_id": "s1", "task_id": None, "tool_call_id": "setup-1"}
        self.assertEqual(policy.pre_tool_call(host, tool_name="kumi_setup", args={"action": "install"}, **hook)["action"],
                         "approve")
        policy.post_approval_response(host, tool_call_id="setup-1", choice="once")

        elsewhere_library = Path(tempfile.gettempdir()) / "kumi-setup-tool-tests" / "Other Library"

        def elsewhere(args: dict) -> dict:  # another chat's runtime sees other paths
            return {"daw": "ableton", "component": "Live Remote Script", "version": "1.0.105", "state": "missing",
                    "paths": [str(elsewhere_library / "Remote Scripts" / "AbletonMcpBridge")], "owner": None,
                    "after": None,
                    "effects": [f"Copy the AbletonMcpBridge Remote Script into {elsewhere_library / 'Remote Scripts'}"]}

        setup.plan("ableton", setup_daw=elsewhere, data_dir=data, plugin_dir=host.plugin_dir)
        result = support.handle(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertEqual(result["error"]["code"], "plan_changed")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, setup_daw_calls(data))

    def test_an_approved_install_that_hermes_then_blocks_leaves_nothing_to_run(self):
        host, _, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        hook = {"session_id": "s1", "task_id": None, "tool_call_id": "setup-blocked"}
        self.assertEqual(policy.pre_tool_call(host, tool_name="kumi_setup", args={"action": "install"}, **hook)["action"],
                         "approve")
        policy.post_approval_response(host, tool_call_id="setup-blocked", choice="once")
        policy.post_tool_call(host, tool_name="kumi_setup", tool_call_id="setup-blocked", status="blocked")
        result = support.handle(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertEqual(result["error"]["code"], "not_approved")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, setup_daw_calls(data))

    def test_adopting_the_owner_of_lives_component_installs_nothing_and_restarts_the_runtime(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        owner = Path(folder.name)
        host, _, data = desktop_host(self, None, "--component-state", "other_owner", "--component-owner", str(owner),
                                     daw_component=False)
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("owned_elsewhere", "kumi_setup adopt"))
        self.assertIn("blocked", support.call(host, "kumi_setup", {"action": "install"}, session_id="s1"),
                      "a component another Kumi owns is never offered as an install")
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        adopted = support.call(host, "kumi_setup", {"action": "adopt"}, session_id="s1")
        self.assertTrue(adopted["ok"], adopted)
        self.assertEqual(json.loads((data / "runtime" / "current.json").read_text())["kumi_home"], str(owner))
        self.assertIsNone(host.runtimes.get(data), "the next start uses the adopted Kumi home")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, setup_daw_calls(data))

    def test_a_standalone_kumis_part_in_reaper_or_renoise_stops_setup_and_says_how_to_take_it_out(self):
        owner = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory()))) / "Kumi home"
        owner.mkdir()
        host, _, data = desktop_host(self, None, "--component-state", "other_owner", "--component-owner", str(owner),
                                     daw_component=False)
        for daw, name in (("reaper", "REAPER"), ("renoise", "Renoise")):
            with self.subTest(daw):
                host.runtimes.stop_all()  # each DAW's dry run takes a runtime of its own
                plan = support.call(host, "kumi_setup", {"action": "plan", "daw": daw}, session_id="s1")
                self.assertEqual((plan["step"], plan["next"]), ("other_kumi", None), "never adopt (kumi-adv's D1)")
                self.assertIn("can't share it", plan["summary"])
                self.assertNotIn(str(owner), json.dumps(plan), "no path reaches the model")
                for action in ("install", "adopt"):
                    self.assertIsNone(setup.approval(daw, action=action, data_dir=data), "nothing to approve")
                    blocked = policy.pre_tool_call(host, "kumi_setup", {"action": action, "daw": daw},
                                                   session_id="s1", tool_call_id=f"c-{action}")
                    self.assertEqual(blocked["action"], "block")
                    self.assertIn("uninstalls that Kumi", blocked["message"], "and the model can say why")
                    self.assertNotIn(str(owner), blocked["message"])
                host.runtimes.stop_all()
                out = Tty()
                self.assertEqual(commands.cli_setup(host, daw, stdin=Tty("y\n"), stdout=out), 1)
                self.assertIn(f"belongs to the standalone Kumi at {owner}", out.getvalue())
                launcher = owner / "bin" / ("kumi.cmd" if os.name == "nt" else "kumi")  # install.ps1's, on Windows
                self.assertIn(f'quit {name}, then run "{launcher}" uninstall in a terminal', out.getvalue(),
                              "the exact way out, quoted for the space in its folder")
                self.assertNotIn("Go ahead?", out.getvalue(), "nothing is asked")
                self.assertNotIn({"daw": daw, "dry_run": False}, setup_daw_calls(data), "nothing was installed")
        current = data / "runtime" / "current.json"
        self.assertNotEqual(json.loads(current.read_text()).get("kumi_home") if current.exists() else None, str(owner),
                            "nor that Kumi adopted")

    def test_the_first_daw_set_up_becomes_kumis_daw_in_the_same_approval(self):
        host, ctx, data = desktop_host(self, daw_component=False)  # the DAW setting says Live; nothing is set up
        plan = support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s1")
        self.assertEqual((plan["step"], plan["use_daw"]), ("daw_component", True))
        asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1",
                                     tool_call_id="c1")
        self.assertIn("Make REAPER Kumi's DAW in this profile", asked["message"], "the producer approves that too")
        policy.post_approval_response(host, tool_call_id="c1", choice="once")
        installed = support.handle(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertEqual(ctx.settings["daw"], "reaper", "kumi_connect now goes to REAPER")
        self.assertEqual(host.runtimes.get(data).daw, "reaper", "the runtime that set it up already works in it")

    def test_a_second_daw_set_up_leaves_the_one_set_up_as_kumis_daw_unless_asked(self):
        host, ctx, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertTrue(support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")["ok"])  # Live
        host.runtimes.stop_all()  # its chat ended
        plan = support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s2")
        self.assertEqual((plan["step"], plan["use_daw"]), ("daw_component", False))
        installed = support.call(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s2")
        self.assertTrue(installed["ok"], installed)
        self.assertEqual(ctx.settings["daw"], "ableton", "Live, which is set up, stays Kumi's DAW")
        self.assertIn("hermes kumi setup --daw reaper --use", installed["message"], "the exact next step")
        out = Tty()
        self.assertEqual(commands.cli_setup(host, "reaper", use=True, stdin=Tty("y\n"), stdout=out), 0, out.getvalue())
        self.assertIn("Kumi: make REAPER Kumi's DAW in this profile", out.getvalue(), "asked first")
        self.assertEqual(ctx.settings["daw"], "reaper", "asked for, and approved")

    def test_the_switch_is_part_of_what_was_approved(self):
        host, ctx, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s1")  # makes REAPER Kumi's
        hook = {"session_id": "s1", "task_id": None, "tool_call_id": "c1"}
        self.assertEqual(policy.pre_tool_call(host, "kumi_setup", {"action": "install", "daw": "reaper"}, **hook)
                         ["action"], "approve")
        policy.post_approval_response(host, tool_call_id="c1", choice="once")
        ctx.settings["daw"] = "reaper"  # the setting changed after the approval: the plan would no longer switch it
        result = support.handle(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1")
        self.assertEqual(result["error"]["code"], "plan_changed")
        self.assertNotIn({"daw": "reaper", "dry_run": False}, setup_daw_calls(data), "nothing installed")

    def test_a_daw_already_in_place_is_made_kumis_daw_by_a_step_of_its_own(self):
        host, ctx, data = desktop_host(self, None, "--component-state", "same", daw_component=False)
        plan = support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("use_daw", "kumi_setup install"))
        asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1",
                                     tool_call_id="c1")
        self.assertEqual(asked["message"].splitlines()[0], "Kumi: make REAPER Kumi's DAW in this profile")
        policy.post_approval_response(host, tool_call_id="c1", choice="once")
        self.assertTrue(support.handle(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1")
                        ["ok"])
        self.assertEqual(ctx.settings["daw"], "reaper")
        self.assertNotIn({"daw": "reaper", "dry_run": False}, setup_daw_calls(data), "nothing was installed for it")

    def test_a_daw_set_up_beside_kumis_daw_says_how_to_switch_and_changes_nothing_unasked(self):
        host, ctx, _ = desktop_host(self, None, "--component-state", "same")  # Live and REAPER are both set up
        plan = support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s1")
        self.assertEqual(plan["step"], "none")
        self.assertIn(f"still connects to {setup.DAWS['ableton']}", plan["next"])
        self.assertIn("hermes kumi setup --daw reaper --use", plan["next"])
        host.runtimes.stop_all()
        out = Tty()
        self.assertEqual(commands.cli_setup(host, "reaper", use=True, stdin=Tty("n\n"), stdout=out), 1)
        self.assertIn("Nothing was changed.", out.getvalue())
        self.assertEqual(commands.cli_setup(host, "reaper", use=True, stdin=io.StringIO("y\n"),
                                            stdout=io.StringIO()), 1, "no terminal: nothing is asked")
        self.assertEqual(ctx.settings["daw"], "ableton")

    def test_a_setting_kumi_cant_write_is_said_and_never_asked_again(self):
        host, ctx, data = desktop_host(self, daw_component=False)
        support.call(host, "kumi_setup", {"action": "plan", "daw": "reaper"}, session_id="s1")
        with mock.patch.object(ctx, "set_config", side_effect=PermissionError("a managed install's settings")):
            installed = support.call(host, "kumi_setup", {"action": "install", "daw": "reaper"}, session_id="s1")
        self.assertTrue(installed["ok"], "REAPER's part is installed all the same")
        self.assertNotIn("use_daw", installed)
        self.assertIn("couldn't change its DAW setting", installed["message"])
        self.assertEqual(ctx.settings["daw"], "ableton")
        out = Tty()
        with mock.patch.object(ctx, "set_config", side_effect=PermissionError("a managed install's settings")):
            self.assertEqual(commands.cli_setup(host, "reaper", stdin=Tty("y\ny\ny\n"), stdout=out), 1)
        self.assertEqual(out.getvalue().count("Go ahead?"), 1, "asked once, then stopped")

    def test_installing_the_runtime_downloads_the_pinned_bundle_and_stops_the_older_runtime(self):
        host, _, data = desktop_host(self)  # Kumi 1.24.66's runtime, set up
        support.call(host, "kumi_connect", {}, session_id="s1")
        older = host.runtimes.get(data)
        self.assertIsNotNone(older, "the older runtime runs")
        newer, target = "1.24.67", setup.host_target()
        bundle = io.BytesIO()
        with tarfile.open(fileobj=bundle, mode="w:xz") as archive:
            info = tarfile.TarInfo(setup.executable(target))
            info.size, info.mode = 3, 0o755
            archive.addfile(info, io.BytesIO(b"#!\n"))
        data_bytes = bundle.getvalue()
        support.pin(host.plugin_dir, version=newer)
        pin = json.loads((host.plugin_dir / "kumi_release.json").read_text())
        pin["targets"][target].update(sha256=hashlib.sha256(data_bytes).hexdigest(), size=len(data_bytes))
        (host.plugin_dir / "kumi_release.json").write_text(json.dumps(pin))
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual(plan["step"], "runtime", "this plugin now pins a newer Kumi")
        with mock.patch.object(setup, "_open_url", side_effect=lambda url: io.BytesIO(data_bytes)):
            installed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertIsNone(host.runtimes.get(data), "the older runtime is stopped: the next start is the newer one")
        self.assertTrue(support.eventually(lambda: not older.client.running), "its process ended")
        self.assertEqual(setup.runtime_state(data, newer, target)[0], "ready")

    def test_allowing_an_ffmpeg_restarts_the_runtime_with_it(self):
        host, _, data = desktop_host(self, None, "--component-state", "same")  # set up for Live already
        support.call(host, "kumi_connect", {}, session_id="s1")
        running = host.runtimes.get(data)
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        program = Path(folder.name) / ("ffmpeg.exe" if setup.os.name == "nt" else "ffmpeg")
        program.write_bytes(b"#!/bin/sh\n")
        program.chmod(0o755)
        with mock.patch.object(setup, "_ffmpeg_search", return_value=[folder.name]):
            plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
            self.assertEqual((plan["step"], plan["optional"], plan["next"]),
                             ("ffmpeg", True, "kumi_setup install (optional: ask the producer; setup is done either way)"))
            self.assertIn(f"Kumi's runtime will run {setup.os.path.realpath(program)}",
                          setup.approval("ableton", data_dir=data)["message"])
            installed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertIsNone(host.runtimes.get(data), "stopped, so the next start passes --tool ffmpeg")
        self.assertTrue(support.eventually(lambda: not running.client.running), "its process ended")
        self.assertEqual(installed["status"]["compressed_audio"]["state"], "available")
        argv = runtime.serve_argv(host.install(data), data / "state", "ableton")
        self.assertEqual(argv[-2:], ["--tool", f"ffmpeg={setup.os.path.realpath(program)}"],
                         "what setup recorded is what the next start passes (runtime.py, #962)")
        program.write_bytes(b"#!/bin/sh\necho upgraded\n")
        self.assertNotIn("--tool", runtime.serve_argv(host.install(data), data / "state", "ableton"))
        self.assertEqual(setup.status(daw="ableton", data_dir=data, plugin_dir=host.plugin_dir)
                         ["compressed_audio"]["state"], "changed", "setup and runtime.py agree it changed")


class ModelAnswers(unittest.TestCase):
    """What kumi_status and kumi_setup give the model names no path on this computer, nor a URL (adversarial review LOW
    11, rgzm): the approval and `hermes kumi status` show the producer every one."""

    PATHLIKE = re.compile(r"(?<![\w-])/[^\s\"']*/|[A-Za-z]:[\\/]|\\\\|://")  # `/a/b`, `C:\`, `\\server`, a URL

    def assertNoPath(self, answer: dict, *known: Path) -> None:
        said = json.dumps(answer, ensure_ascii=False)
        self.assertIsNone(self.PATHLIKE.search(said), said)
        for path in known:
            self.assertNotIn(str(path), said)
            self.assertNotIn(os.path.realpath(path), said)

    def test_every_step_of_setup_and_kumi_status_answer_without_a_path(self):
        host, _, data = desktop_host(self, installed=False)
        self.assertNoPath(support.call(host, "kumi_status", {}, session_id="s1"), data, host.plugin_dir)
        self.assertNoPath(support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1"), data)

        host, _, data = desktop_host(self)  # a newer Kumi to install: the runtime step
        bundle = io.BytesIO()
        with tarfile.open(fileobj=bundle, mode="w:xz") as archive:
            info = tarfile.TarInfo(setup.executable(setup.host_target()))
            info.size, info.mode = 3, 0o755
            archive.addfile(info, io.BytesIO(b"#!\n"))
        support.pin(host.plugin_dir, version="1.24.67")
        pin = json.loads((host.plugin_dir / "kumi_release.json").read_text())
        pin["targets"][setup.host_target()].update(sha256=hashlib.sha256(bundle.getvalue()).hexdigest(),
                                                   size=len(bundle.getvalue()))
        (host.plugin_dir / "kumi_release.json").write_text(json.dumps(pin))
        self.assertNoPath(support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1"), data)
        with mock.patch.object(setup, "_open_url", side_effect=lambda url: io.BytesIO(bundle.getvalue())):
            installed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertNoPath(installed, data)

        host, _, data = desktop_host(self, daw_component=False)  # the DAW step, then its install
        self.assertNoPath(support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1"), data)
        installed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(installed["ok"], installed)
        self.assertNoPath(installed, data)
        support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertNoPath(support.call(host, "kumi_status", {}, session_id="s1"), data)

        folder = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        program = folder / ("ffmpeg.exe" if os.name == "nt" else "ffmpeg")
        program.write_bytes(b"#!/bin/sh\n")
        program.chmod(0o755)
        host, _, data = desktop_host(self, None, "--component-state", "same")  # ffmpeg, the optional last step
        with mock.patch.object(setup, "_ffmpeg_search", return_value=[str(folder)]):
            self.assertNoPath(support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1"), data, folder)
            allowed = support.call(host, "kumi_setup", {"action": "install"}, session_id="s1")
        self.assertTrue(allowed["ok"], allowed)
        self.assertNoPath(allowed, data, folder)

        owner = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        host, _, data = desktop_host(self, None, "--component-state", "other_owner", "--component-owner", str(owner),
                                     daw_component=False)
        self.assertNoPath(support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1"), data, owner)
        adopted = support.call(host, "kumi_setup", {"action": "adopt"}, session_id="s1")
        self.assertTrue(adopted["ok"], adopted)
        self.assertNoPath(adopted, data, owner)

    def test_a_setup_failure_whose_words_name_a_path_is_said_without_it(self):
        owner = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        host, _, data = desktop_host(self, None, "--component-state", "other_owner", "--component-owner",
                                     str(owner / "Kumi"), daw_component=False)
        (owner / "Kumi").mkdir()
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        (owner / "Kumi").rmdir()  # the installation to adopt went away after it was approved
        failed = support.call(host, "kumi_setup", {"action": "adopt"}, session_id="s1")
        self.assertEqual(failed["error"]["code"], "not_found")
        self.assertNoPath(failed, data, owner)



class Cli(unittest.TestCase):
    def test_it_asks_installs_the_daw_component_and_stops_the_runtime_it_started(self):
        host, _, data = desktop_host(self, daw_component=False)
        out = Tty()
        self.assertEqual(commands.cli_setup(host, "ableton", stdin=Tty("y\n"), stdout=out), 0, out.getvalue())
        said = out.getvalue()
        self.assertIn("Kumi Ears.amxd", said)
        self.assertIn("Go ahead? [y/N]", said)
        dry, real = {"daw": "ableton", "dry_run": True}, {"daw": "ableton", "dry_run": False}
        self.assertEqual(setup_daw_calls(data), [dry, dry, real, dry], "plan, re-plan, install, then nothing left")
        self.assertIsNone(host.runtimes.get(data), "the command stops the runtime it started")
        state = setup.status(daw="ableton", data_dir=data, plugin_dir=host.plugin_dir)
        self.assertEqual((state["state"], state["daw_component"]["state"]), ("ready", "installed"))
        self.assertEqual([part["name"] for part in state["daw_component"]["parts"]], ["Remote Script", "Kumi Ears"])

    def test_a_no_changes_nothing_and_no_terminal_runs_nothing(self):
        host, _, data = desktop_host(self, daw_component=False)
        out = Tty()
        self.assertEqual(commands.cli_setup(host, "ableton", stdin=Tty("n\n"), stdout=out), 1)
        self.assertIn("Nothing was changed.", out.getvalue())
        self.assertEqual(setup_daw_calls(data), [{"daw": "ableton", "dry_run": True}])
        out = io.StringIO()
        self.assertEqual(commands.cli_setup(host, "ableton", stdin=io.StringIO("y\n"), stdout=out), 1)
        self.assertIn("needs a terminal", out.getvalue())
        receipt = json.loads((data / "setup" / "receipt.json").read_text()) if (data / "setup" / "receipt.json").exists() else {}
        self.assertNotIn("ableton", receipt.get("daw_components") or {})


if __name__ == "__main__":
    unittest.main()
