"""Kumi's Automation Blueprints (W4): fill-in-the-blanks Hermes cron jobs a producer can schedule from `/blueprint`,
the dashboard's Blueprints tab or Desktop's Cron page, listed as `kumi:<key>`.

Each only reads: `kumi_history` or `kumi_status`, which Kumi's approval policy asks nothing for, so it runs unattended.
None may start Kumi, connect to the DAW, change or undo anything, scan or listen: the prompt says so, and W2's policy
refuses `kumi_connect` and `kumi_setup` inside a cron run. When Kumi isn't running, a job says so in one line and stops.
"""
from __future__ import annotations

from typing import Any

# What a scheduled run must never call: each starts Kumi, takes the DAW's lease, asks for an approval nobody is there to
# give, or changes something.
NEVER = "kumi_connect, kumi_setup, kumi_library, kumi_listen, kumi_change or kumi_undo"

_DELIVER = {"name": "deliver", "type": "enum", "label": "Where to deliver?", "default": "origin",
            "options": ["origin", "local", "telegram", "discord", "email"], "strict": False}

BLUEPRINTS: tuple[dict[str, Any], ...] = (
    {
        "key": "weekly-digest",
        "title": "Kumi: the week in your DAW",
        "description": "Once a week, what Kumi changed in your DAW projects: the changes, what was undone, and anything "
                       "it wasn't sure of. It reads Kumi's HISTORY only and changes nothing.",
        "category": "weekly",
        "schedule_template": "{minute} {hour} * * {dow}",
        "prompt_template": (
            "Summarize what Kumi changed in my DAW projects over the last 7 days, from Kumi's HISTORY: call "
            "kumi_history, and no other Kumi tool but kumi_status. Per project: what changed, what was undone, and "
            "any change marked unsure or partly applied, with what to check in the project. If Kumi isn't running "
            "(kumi_history says to connect first), say so in one line and stop. Never call " + NEVER + ": this runs "
            "unattended, and none of them may run without me. If nothing changed, say so in one line."
        ),
        "slots": [
            {"name": "time", "type": "time", "label": "What time?", "default": "18:00", "help": "24h local time"},
            {"name": "day", "type": "enum", "label": "Which day?", "default": "friday",
             "options": ["friday", "saturday", "sunday", "monday"]},
            _DELIVER,
        ],
        "tags": ["kumi", "music", "weekly"],
    },
    {
        "key": "morning-check",
        "title": "Kumi: is the studio ready?",
        "description": "On weekday mornings, whether Kumi is set up and can reach your DAW, and the one thing to do "
                       "if not. It reads Kumi's status only and changes nothing.",
        "category": "daily",
        "schedule_template": "{minute} {hour} * * 1-5",
        "prompt_template": (
            "Check whether Kumi is ready for today's session: call kumi_status, and no other Kumi tool. In two lines "
            "at most, say whether Kumi is set up, whether it can read compressed audio, and the one thing to do if "
            "something needs doing (run hermes kumi setup, or open a chat and ask Kumi to connect to the DAW). Kumi "
            "not running yet is normal: just say it starts when a chat connects. Never call " + NEVER + ": this runs "
            "unattended."
        ),
        "slots": [
            {"name": "time", "type": "time", "label": "What time?", "default": "09:30", "help": "24h local time"},
            _DELIVER,
        ],
        "tags": ["kumi", "music", "daily"],
    },
)


def register(ctx: Any) -> None:
    """Add Kumi's blueprints to this profile's catalog. A Hermes without blueprints gets none, and its plugin loads as
    before."""
    add = getattr(ctx, "register_automation_blueprint", None)
    if add is None:
        return
    for blueprint in BLUEPRINTS:
        fields = dict(blueprint)
        add(fields.pop("key"), **fields)
