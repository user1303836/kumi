"""The tools' schemas as Hermes registers them (RESEARCH.md C1): what the model reads to choose and call a tool."""
from __future__ import annotations

# Kumi's names for the DAWs it works in (its settings' and the runtime's): `ableton` is Ableton Live.
DAWS = ("ableton", "reaper", "renoise")
DAW_NAMES = {"ableton": "Live", "reaper": "REAPER", "renoise": "Renoise"}

KUMI_STATUS = {
    "name": "kumi_status",
    "description": (
        "Kumi's state in this Hermes profile: whether Kumi is set up, whether its runtime is running and connected to "
        "the DAW (Ableton Live, REAPER or Renoise), the open project, which chat holds the DAW, and what Kumi supports "
        "there. Call it first, and whenever another Kumi tool says Kumi isn't set up or connected. It only reads: it "
        "never starts Kumi or touches the DAW."
    ),
    "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
}

KUMI_CONNECT = {
    "name": "kumi_connect",
    "description": (
        "Connect Kumi to the DAW: Ableton Live (ableton), REAPER or Renoise. Starts Kumi's runtime for this chat when "
        "it isn't running (Kumi's local program and its DAW bridge, as background processes on this computer, until "
        "the chat ends); changes nothing in the DAW or the project. The DAW must be open with Kumi's component enabled "
        "(Live: the AbletonMcpBridge control surface). Returns what kumi_status returns."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "daw": {"type": "string", "enum": list(DAWS),
                    "description": "The DAW to connect to; the plugin's DAW setting when left out."},
        },
        "additionalProperties": False,
    },
}
