"""Kumi's Automation Blueprints, as Hermes's own catalog builds and schedules them. That a scheduled run can't start
Kumi is the policy's to enforce, and its test (#983)."""
from __future__ import annotations

import unittest

import support

blueprints = support.module("blueprints")


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
