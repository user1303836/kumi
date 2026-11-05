"""kumi_setup and `hermes kumi setup` (C8) through the plugin, against the fixture runtime's setup_daw: what the model
can plan, what the producer is asked, and the terminal command."""
from __future__ import annotations

import argparse
import io
import json
import unittest

import support

kumi = support.plugin()
commands = support.module("commands")
policy = support.module("policy")
setup = support.module("setup")


class Tty(io.StringIO):
    def isatty(self) -> bool:
        return True


def setup_daw_calls(data) -> list[dict]:
    """The setup_daw calls the fixture runtime received, without Hermes's `_host`."""
    return [{key: value for key, value in message["params"]["arguments"].items() if key != "_host"}
            for message in support.requests(data / "state") if message.get("params", {}).get("name") == "setup_daw"]


class SetupTool(unittest.TestCase):
    def test_planning_the_runtime_names_the_download_and_starts_nothing(self):
        host, _, data = support.host(self, installed=False)
        self.assertIsNone(policy.pre_tool_call(host, "kumi_setup", {"action": "plan"}), "a plan needs no approval")
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("runtime", "kumi_setup install"))
        self.assertIn("https://github.com/user1303836/kumi/releases/download/", plan["effects"][0])
        self.assertIsNone(host.runtimes.get(data), "planning the runtime step starts no runtime")

    def test_the_daw_step_is_planned_from_the_runtimes_dry_run_with_its_exact_paths(self):
        host, _, data = support.host(self, daw_component=False)
        plan = support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        self.assertEqual((plan["step"], plan["next"]), ("daw_component", "kumi_setup install"))
        self.assertTrue(any(path.endswith("Kumi Ears.amxd") for path in plan["paths"]), plan["paths"])
        self.assertEqual(setup_daw_calls(data), [{"daw": "ableton", "dry_run": True}])
        asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1", tool_call_id="c1")
        self.assertEqual(asked["action"], "approve")
        self.assertIn("Kumi Ears.amxd", asked["message"])
        self.assertEqual(asked["rule_key"], f"kumi:setup:{plan['digest']}", "'always' covers this exact plan only")

    def test_install_or_adopt_without_a_plan_is_blocked_before_anyone_is_asked(self):
        host, _, _ = support.host(self, installed=False)
        for action in ("install", "adopt"):
            with self.subTest(action):
                decision = policy.pre_tool_call(host, "kumi_setup", {"action": action}, tool_call_id="c1")
                self.assertEqual(decision, {"action": "block",
                                            "message": "Kumi did not set anything up: run kumi_setup plan first."})
        support.call(host, "kumi_setup", {"action": "plan"})
        self.assertEqual(policy.pre_tool_call(host, "kumi_setup", {"action": "adopt"})["action"], "block",
                         "a runtime plan is never approved as an adoption")

    def test_a_runtime_working_in_another_daw_is_never_asked_to_set_up_this_one(self):
        host, _, data = support.host(self, daw_component=False)
        support.call(host, "kumi_connect", {"daw": "reaper"}, session_id="s1")
        planned = support.call(host, "kumi_setup", {"action": "plan", "daw": "ableton"}, session_id="s2")
        self.assertEqual(planned["error"]["code"], "other_daw")
        self.assertEqual(setup_daw_calls(data), [])


class Cli(unittest.TestCase):
    def test_hermes_kumi_setup_is_a_subcommand_with_an_optional_daw(self):
        ctx = support.Context()
        kumi.register(ctx)
        parser = argparse.ArgumentParser()
        ctx.cli["kumi"]["setup_fn"](parser)
        args = parser.parse_args(["setup", "--daw", "reaper"])
        self.assertEqual((args.kumi_action, args.daw), ("setup", "reaper"))
        with self.assertRaises(SystemExit):
            parser.parse_args(["setup", "--daw", "live"])

    def test_it_asks_installs_the_daw_component_and_stops_the_runtime_it_started(self):
        host, _, data = support.host(self, daw_component=False)
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
        host, _, data = support.host(self, daw_component=False)
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
