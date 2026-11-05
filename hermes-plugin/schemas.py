"""The tools' schemas as Hermes registers them (RESEARCH.md C1): what the model reads to choose and call a tool."""
from __future__ import annotations

# Kumi's names for the DAWs it works in (its settings' and the runtime's): `ableton` is Ableton Live.
DAWS = ("ableton", "reaper", "renoise")
DAW_NAMES = {"ableton": "Live", "reaper": "REAPER", "renoise": "Renoise"}
MAX_STEPS = 20


def _tool(name: str, description: str, properties: dict | None = None, required: tuple[str, ...] = ()) -> dict:
    parameters = {"type": "object", "properties": properties or {}, "additionalProperties": False}
    if required:
        parameters["required"] = list(required)
    return {"name": name, "description": description, "parameters": parameters}


_SOURCE = {
    "type": "object",
    "description": ("One audio file Kumi may read: {\"sound_id\", \"map_revision\"} from kumi_sounds, {\"artifact_id\"} from an "
                    "earlier Kumi result, or {\"file\": \"/absolute/path\"} inside an approved sample folder or this "
                    "profile's media."),
    "properties": {"file": {"type": "string"}, "sound_id": {"type": "string"}, "map_revision": {"type": "string"},
                   "artifact_id": {"type": "string"}},
    "additionalProperties": False,
}

KUMI_STATUS = _tool(
    "kumi_status",
    "Kumi's state in this Hermes profile: whether Kumi is set up, whether its runtime is running and connected to the "
    "DAW (Ableton Live, REAPER or Renoise), the open project, which chat holds the DAW, and what Kumi supports there. "
    "Call it first, and whenever another Kumi tool says Kumi isn't set up or connected. It only reads: it never starts "
    "Kumi or touches the DAW.")

KUMI_CONNECT = _tool(
    "kumi_connect",
    "Connect Kumi to the DAW: Ableton Live (ableton), REAPER or Renoise. Starts Kumi's runtime for this chat when it "
    "isn't running (Kumi's local program and its DAW bridge, as background processes on this computer, until the chat "
    "ends); changes nothing in the DAW or the project. The DAW must be open with Kumi's component enabled (Live: the "
    "AbletonMcpBridge control surface). Returns what kumi_status returns.",
    {"daw": {"type": "string", "enum": list(DAWS), "description": "The DAW to connect to; the DAW setting when left out."}})

KUMI_OBSERVE = _tool(
    "kumi_observe",
    "Look at the open project now: FOCUS (the track, device and clip the producer is on), what changed since Kumi last "
    "looked (catch_up), or a bounded overview of the whole project. Returns text, the project key and epoch, and refs "
    "for its tracks and devices. Refs and the epoch are what kumi_change needs; they go stale when the project changes, "
    "so observe again rather than reuse old ones. Reads only: it writes nothing to the project.",
    {"scope": {"type": "string", "enum": ["focus", "catch_up", "project"],
               "description": "focus: what the producer is on; catch_up: changes since Kumi last looked; project: the "
                              "whole project, bounded."},
     "max_chars": {"type": "integer", "minimum": 200, "maximum": 4000, "description": "Longest text to return."}},
    ("scope",))

KUMI_READ = _tool(
    "kumi_read",
    "Read rows from the open project in the DAW's own read schema (tracks, devices, clips, parameters, with refs and a "
    "cursor for more), or the input schema of one step kumi_change accepts. Use it to find the exact refs and current "
    "values a change needs. Reads only; never runs code.",
    {"action": {"type": "string", "enum": ["data", "schema"]},
     "query": {"type": "object", "description": "action data: what to read, in the DAW's read schema."},
     "tool": {"type": "string", "description": "action schema: the step to describe, e.g. set_track_volume."}},
    ("action",))

KUMI_CHANGE = _tool(
    "kumi_change",
    "Change the open project with a bounded, typed plan of up to 20 steps, each one of the DAW's own change steps "
    "(look one up with kumi_read action schema), as one entry in Kumi's HISTORY. The producer approves the exact change "
    "before it runs. Pass the project key and epoch from your latest kumi_observe or kumi_read: a changed project is "
    "refused, not guessed. Placing a sound, a generated file or a library sample on a track is an import_audio step. "
    "Returns applied, partial, unsure or refused, with HISTORY ids; never retry an unsure change, check kumi_history.",
    {"title": {"type": "string", "maxLength": 120,
               "description": "What the change does, in a few words the producer will see."},
     "steps": {"type": "array", "minItems": 1, "maxItems": MAX_STEPS, "items": {
         "type": "object",
         "properties": {"tool": {"type": "string"}, "input": {"type": "object"},
                        "as": {"type": "string", "description": "A name later steps can use for this step's result."}},
         "required": ["tool", "input"], "additionalProperties": False}},
     "expected_project_key": {"type": "string"},
     "expected_epoch": {"type": "string"},
     "note": {"type": "string", "maxLength": 500, "description": "Why, kept with the HISTORY entry."}},
    ("title", "steps", "expected_project_key", "expected_epoch"))

KUMI_UNDO = _tool(
    "kumi_undo",
    "Undo one change Kumi made, by its HISTORY id from kumi_history or kumi_change. The producer approves it first. "
    "Returns undone, kept (the producer changed it since, so it was left), unsure, expired or refused. Undo only from "
    "HISTORY, never from memory of what the project was.",
    {"history_id": {"type": "string"}}, ("history_id",))

KUMI_HISTORY = _tool(
    "kumi_history",
    "Kumi's HISTORY for the open project, newest first: each change's id, title, family, time, state and whether it can "
    "still be undone. Reads only.",
    {"limit": {"type": "integer", "minimum": 1, "maximum": 200}})

# A listen's source: a file, a sound or an artifact as _SOURCE, or one track or the mix over a beat window (CC-2).
_LISTEN_SOURCE = {
    "type": "object",
    "description": (_SOURCE["description"] + " Or a track of the open project, or its mix, over a beat window: "
                    "{\"track\": {\"ref\": \"<track ref from kumi_observe>\"} or {\"mix\": true}, \"from_beat\", "
                    "\"beats\"}. That plays the window in the DAW (60 s at most, only while it's stopped), so the "
                    "producer approves it."),
    "properties": {**_SOURCE["properties"],
                   "track": {"type": "object",
                             "description": "{\"ref\": \"…\"} for one track, {\"mix\": true} for the mix.",
                             "properties": {"ref": {"type": "string"}, "mix": {"type": "boolean"}},
                             "additionalProperties": False},
                   "from_beat": {"type": "number", "minimum": 0,
                                 "description": "The beat to start at (0 is the first)."},
                   "beats": {"type": "number", "minimum": 0, "maximum": 256,
                             "description": "How many beats to listen to: more than 0, at most 256 and 60 s."}},
    "additionalProperties": False,
}

KUMI_LISTEN = _tool(
    "kumi_listen",
    "Measure audio: a file on this computer, or a track or the mix of the open project over a beat window: loudness "
    "(LUFS), loudness range, true peak, spectral balance and centroid, tempo and key, and, with compare_to, the "
    "differences from a file. Returns the measurements and the audio as MEDIA:. Nothing is heard: these are "
    "measurements, so report them as such. A file only reads; a track or the mix plays that window in the DAW while "
    "it's stopped, and the producer approves it first. For a track or the mix, pass the project key and epoch "
    "kumi_observe gave, and refs from it.",
    {"source": _LISTEN_SOURCE, "compare_to": _SOURCE,
     "expected_project_key": {"type": "string", "description": "For a track or the mix: kumi_observe's project_key."},
     "expected_epoch": {"type": "string", "description": "For a track or the mix: kumi_observe's epoch."}},
    ("source",))

KUMI_SOUNDS = _tool(
    "kumi_sounds",
    "Search the producer's sound map (samples in the folders they approved) by words, or find sounds near one sound. "
    "Returns sound ids for the map's current revision; pass them to kumi_listen, or place one with a kumi_change "
    "import_audio step. Reads only.",
    {"query": {"type": "string"}, "near": {"type": "string", "description": "A sound id to find similar sounds to."},
     "kind": {"type": "string", "description": "A kind of sound, e.g. kick, snare, pad, loop."},
     "limit": {"type": "integer", "minimum": 1, "maximum": 100}},
    ("query",))

KUMI_LIBRARY = _tool(
    "kumi_library",
    "The producer's sample library: its folders and scan state (status), add a folder Kumi may read (add_folder, which "
    "the producer approves), or scan the approved folders to build the sound map (scan, which the producer approves; it "
    "runs as a job: follow it with kumi_job). Kumi reads only folders added here.",
    {"action": {"type": "string", "enum": ["status", "add_folder", "scan"]},
     "folder": {"type": "string", "description": "add_folder: an absolute folder path."}},
    ("action",))

KUMI_JOB = _tool(
    "kumi_job",
    "The state, progress and result of a long Kumi job (a library scan), by the job id the tool that started it returned. "
    "Reads only.",
    {"job_id": {"type": "string"}}, ("job_id",))
