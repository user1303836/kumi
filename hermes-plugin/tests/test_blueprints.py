"""Kumi's Automation Blueprints: what a producer can schedule, and that a scheduled run can only read."""
from __future__ import annotations

import re
import unittest

import support

blueprints = support.module("blueprints")

# Every Kumi tool that starts Kumi, takes the DAW's lease, needs an approval or changes something.
WRITERS = ("kumi_connect", "kumi_setup", "kumi_library", "kumi_listen", "kumi_change", "kumi_undo")


class Registering(unittest.TestCase):
    def test_both_blueprints_are_added_and_an_older_hermes_gets_none(self):
        added = []

        class Ctx:
            def register_automation_blueprint(self, key, **fields):
                added.append((key, fields["schedule_template"]))

        blueprints.register(Ctx())
        self.assertEqual(added, [("weekly-digest", "{minute} {hour} * * {dow}"), ("morning-check", "{minute} {hour} * * 1-5")])
        blueprints.register(object())  # no register_automation_blueprint: nothing, and no error

    def test_a_scheduled_run_may_call_only_the_read_tools_and_is_told_never_to_call_a_writer(self):
        for blueprint in blueprints.BLUEPRINTS:
            prompt = blueprint["prompt_template"]
            # The tools it may call: named outside the never-call list.
            allowed = set(re.findall(r"kumi_[a-z]+", prompt.replace(blueprints.NEVER, "")))
            self.assertLessEqual(allowed, {"kumi_history", "kumi_status"}, blueprint["key"])
            self.assertIn("Never call " + blueprints.NEVER, prompt)
            for tool in WRITERS:
                self.assertIn(tool, blueprints.NEVER)


class ThroughHermes(unittest.TestCase):
    """Hermes's own catalog (v0.22.7, `cron.blueprint_plugins`) accepts them and fills them into schedules."""

    def test_hermes_builds_each_blueprint_and_fills_its_defaults_into_a_schedule(self):
        try:
            from cron.blueprint_catalog import fill_blueprint
            from cron.blueprint_plugins import build_plugin_blueprint  # only where Hermes is importable
        except ImportError:
            self.skipTest("Hermes isn't importable here")
        schedules = {}
        for blueprint in blueprints.BLUEPRINTS:
            fields = dict(blueprint)
            built = build_plugin_blueprint("kumi", fields.pop("key"), **fields)
            schedules[built.key] = fill_blueprint(built, {})["schedule"]
        self.assertEqual(schedules, {"kumi:weekly-digest": "0 18 * * 5", "kumi:morning-check": "30 9 * * 1-5"})
        weekly = dict(blueprints.BLUEPRINTS[0])
        built = build_plugin_blueprint("kumi", weekly.pop("key"), **weekly)
        self.assertEqual(fill_blueprint(built, {"time": "07:15", "day": "monday"})["schedule"], "15 7 * * 1")


if __name__ == "__main__":
    unittest.main()
