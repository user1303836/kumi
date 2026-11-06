"""The tools' schemas as Hermes registers them (RESEARCH.md C1): what the model reads to choose and call a tool."""
from __future__ import annotations

import json
from pathlib import Path

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

KUMI_SETUP = _tool(
    "kumi_setup",
    "Set Kumi up in this Hermes profile, one approved step at a time. `plan` says the next step and exactly what it "
    "will do: download Kumi's runtime (the release this plugin pins, from Kumi's GitHub releases, checked against its "
    "SHA-256) into this profile's plugin data, or install Kumi's part in the DAW at the exact paths it names. Planning "
    "the DAW step starts Kumi's runtime for a dry run, which changes nothing. `install` does the planned step, and the "
    "producer is asked first; plan again after each step until the plan says nothing is left. When a standalone Kumi "
    "already put its part in Live, the plan says so, and `adopt` uses that installation as it is instead of replacing "
    "it (also asked first). A standalone Kumi's part in REAPER or Renoise can't be shared: the plan says so, and the "
    "producer takes it out with that Kumi before setup can go on. Once the DAW is set up, if an ffmpeg is already on "
    "this computer, the plan offers one optional last step: letting Kumi's runtime run it to read MP3, FLAC and other "
    "compressed audio (Kumi never downloads one; without it, WAV and AIFF only). Ask the producer before installing "
    "that step; setup is done whether or not they allow it, so don't plan again after a no. Setting up the first DAW "
    "also makes it Kumi's DAW, in the same approval. Switching to another DAW that's set up is `hermes kumi setup "
    "--daw <daw> --use` in a terminal, or the DAW setting. Setup never starts or quits the DAW. kumi_status says where "
    "setup stands.",
    {"action": {"type": "string", "enum": ["plan", "install", "adopt"]},
     "daw": {"type": "string", "enum": list(DAWS), "description": "The DAW to set up for; the DAW setting when left out."}},
    ("action",))

KUMI_CONNECT = _tool(
    "kumi_connect",
    "Connect Kumi to the DAW: Ableton Live (ableton), REAPER or Renoise. Starts Kumi's runtime for this chat when it "
    "isn't running (Kumi's local program and its link to the DAW, as background processes on this computer). It runs "
    "until the chat ends, or until it has had nothing to do for 2 hours while connected (15 minutes when it isn't); its "
    "changes can be undone only while it runs. Changes nothing in the DAW or the project. The DAW must be open with "
    "Kumi's part in it enabled (Live: the AbletonMcpBridge control surface). Returns what kumi_status returns.",
    {"daw": {"type": "string", "enum": list(DAWS), "description": "The DAW to connect to; the DAW setting when left out."}})

KUMI_OBSERVE = _tool(
    "kumi_observe",
    "Look at the open project now: FOCUS (the track, device and clip the producer is on), what changed since Kumi last "
    "looked (catch_up), or the whole turn's look (project): what's selected and what changed first, then the steps a "
    "change may use, each with its fields, then the tracks, folded round the one in focus. Returns text, the project "
    "key and epoch, and refs for its tracks and devices. Refs and the epoch are what kumi_change needs; they go stale "
    "when the project changes, so observe again rather than reuse old ones. Refused, in the DAW's words, while the DAW "
    "can't be read (a dialog open, say). Reads only: it writes nothing to the project.",
    {"scope": {"type": "string", "enum": ["focus", "catch_up", "project"],
               "description": "focus: what the producer is on; catch_up: changes since Kumi last looked; project: the "
                              "whole turn's look, bounded."},
     "max_chars": {"type": "integer", "minimum": 200, "maximum": 20000,
                   "description": "Longest text to return (12000 unless given)."}},
    ("scope",))

KUMI_READ = _tool(
    "kumi_read",
    "Read rows from the open project with one of the DAW's readers (tracks, devices, clips, parameters, with refs and "
    "a cursor for more), or the input schema of one step kumi_change accepts or one reader. Use it to find the exact "
    "refs and current values a change needs. action schema without tool lists the steps and the readers (REAPER's "
    "reaper_read, Renoise's renoise_read; Live's are its note reader and its bridge's reads). Reads only; never runs "
    "code.",
    {"action": {"type": "string", "enum": ["data", "schema"]},
     "query": {"type": "object", "description": "action data: the reader's input, as its schema says."},
     "tool": {"type": "string", "description": "action data: the reader to run, e.g. reaper_read or renoise_read "
                                               "(required). action schema: the step or reader to describe, e.g. "
                                               "set_mixer (Live) or track_volume (REAPER, Renoise)."}},
    ("action",))

KUMI_CHANGE = _tool(
    "kumi_change",
    "Change the open project with a bounded, typed plan of up to 20 steps, each one of the DAW's own change steps "
    "(look one up with kumi_read action schema), as one entry in Kumi's HISTORY. The producer approves the exact change "
    "before it runs. Pass the project key and epoch from your latest kumi_observe or kumi_read, or the ones a done "
    "kumi_change returned (its project_key and epoch serve the next change in the same turn): a changed project is "
    "refused, not guessed. A file goes in through the DAW's own file step (Live replace_sample or import_audio, REAPER "
    "item_import, Renoise sample_load), whose file field holds a source: {sound_id, map_revision}, {artifact_id} or "
    "{file, provenance}, never a path. "
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
    "Returns undone, kept (the producer changed it since, so it was left), unsure, expired or refused. A change made "
    "before Kumi's runtime restarted is expired, and its reason says what the producer can do instead. Undo only from "
    "HISTORY, never from memory of what the project was.",
    {"history_id": {"type": "string"}}, ("history_id",))

KUMI_HISTORY = _tool(
    "kumi_history",
    "Kumi's HISTORY for the open project, newest first: each change's id, title, family, time, state and whether it can "
    "still be undone. When Kumi's runtime doesn't run in this Hermes process, and always in a scheduled job, it's "
    "HISTORY as Kumi last published it (from: last_published, as_of): nothing can be undone then, and the project may "
    "have changed since. Reads only; starts nothing. In a scheduled job, its titles and track names go wherever that "
    "job delivers.",
    {"limit": {"type": "integer", "minimum": 1, "maximum": 200}})


def _track_listen_daws() -> tuple[str, ...]:
    """The DAWs whose pinned runtime listens to a track or the mix: kumi_release.json's `listen_sources` (CC-16),
    which scripts/hermes_pin.py reads from that runtime. None on a pin from before them: files only. Read here, beside
    the plugin, since Hermes registers a tool's schema once (and the dashboard loads this file on its own)."""
    try:
        pin = json.loads((Path(__file__).resolve().parent / "kumi_release.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ()
    sources = pin.get("listen_sources") if isinstance(pin, dict) else None
    if not isinstance(sources, dict):
        return ()
    return tuple(daw for daw in DAWS if isinstance(sources.get(daw), list) and "track" in sources[daw])


TRACK_LISTEN_DAWS = _track_listen_daws()
_TRACK_WHERE = " or ".join(DAW_NAMES[daw] for daw in TRACK_LISTEN_DAWS)
# A listen's source: a file, a sound or an artifact as _SOURCE; and, where the pinned runtime takes one, one track or
# the mix over a beat window (CC-2, CC-16).
_LISTEN_SOURCE = {
    "type": "object",
    "description": _SOURCE["description"] + (
        f" Or, in {_TRACK_WHERE}, a track of the open project, or its mix, over a beat window: "
        "{\"track\": {\"ref\": \"<track ref from kumi_observe>\"} or {\"mix\": true}, \"from_beat\", \"beats\"}. "
        "That plays the window in the DAW (60 s at most, only while it's stopped), so the producer approves it."
        if TRACK_LISTEN_DAWS else ""),
    "properties": {**_SOURCE["properties"], **({
        "track": {"type": "object", "description": "{\"ref\": \"…\"} for one track, {\"mix\": true} for the mix.",
                  "properties": {"ref": {"type": "string"}, "mix": {"type": "boolean"}},
                  "additionalProperties": False},
        "from_beat": {"type": "number", "minimum": 0, "description": "The beat to start at (0 is the first)."},
        "beats": {"type": "number", "minimum": 0, "maximum": 256,
                  "description": "How many beats to listen to: more than 0, at most 256 and 60 s."}}
        if TRACK_LISTEN_DAWS else {})},
    "additionalProperties": False,
}

KUMI_LISTEN = _tool(
    "kumi_listen",
    "Measure audio: a file on this computer"
    + (f", or, in {_TRACK_WHERE}, a track or the mix of the open project over a beat window" if TRACK_LISTEN_DAWS
       else " (a bounce, a sample, or a file Kumi made)")
    + ": loudness (LUFS), loudness range, true peak, spectral balance and centroid, tempo and key, and, with "
    "compare_to, the differences from a file. Returns the measurements and the audio as MEDIA:. Nothing is heard: "
    "these are measurements, so report them as such. A file only reads"
    + ("; a track or the mix plays that window in the DAW while it's stopped, and the producer approves it first. "
       "For a track or the mix, pass the project key and epoch kumi_observe gave, and refs from it."
       if TRACK_LISTEN_DAWS else ". This version of Kumi can't listen to a track or the mix in the DAW: to measure "
       "part of the project, ask the producer to bounce it to a file."),
    {"source": _LISTEN_SOURCE, "compare_to": _SOURCE,
     **({"expected_project_key": {"type": "string", "description": "For a track or the mix: kumi_observe's "
                                                                    "project_key."},
         "expected_epoch": {"type": "string", "description": "For a track or the mix: kumi_observe's epoch."}}
        if TRACK_LISTEN_DAWS else {})},
    ("source",))

SOUND_CLASSES = ("kick", "snare", "clap", "hat", "cymbal", "tom", "perc", "drums", "bass", "lead", "pad", "keys",
                 "pluck", "stab", "synth", "guitar", "strings", "brass", "vocal", "fx", "noise", "texture")

KUMI_SOUNDS = _tool(
    "kumi_sounds",
    "Search the producer's sound map (samples in the folders they approved) by words, or find sounds near one sound. "
    "Style words match the indexed names and folders; descriptors such as dark or punchy rank by what Kumi measured. "
    "Filters, by the values Kumi indexed at the last scan: bpm (also matches half or double time), key (also its "
    "relative major or minor, or a one-shot's root note), min_seconds and max_seconds, and class (what the sound is), "
    "beside kind (loop or one-shot). A sound without the indexed value isn't found by its filter. Each hit says why it "
    "matched; a near search adds closeness, 0 to 100 and relative to this map, never a percentage. Returns sound ids "
    "for the map's current revision; pass them to kumi_listen, or place one with a kumi_change file step, its file "
    "field {sound_id, map_revision}. Reads only.",
    {"query": {"type": "string"}, "near": {"type": "string", "description": "A sound id to find similar sounds to."},
     "kind": {"type": "string", "description": "loop or one-shot, or a kind of sound such as kick or pad."},
     "class": {"type": "string", "enum": list(SOUND_CLASSES), "description": "What the sound is, as Kumi indexed it."},
     "bpm": {"type": "number", "exclusiveMinimum": 0, "description": "About this tempo, or half or double it."},
     "key": {"type": "string", "description": "A key such as A minor or F#; its relative key matches too."},
     "min_seconds": {"type": "number", "minimum": 0, "description": "At least this long, in seconds."},
     "max_seconds": {"type": "number", "minimum": 0, "description": "At most this long, in seconds."},
     "limit": {"type": "integer", "minimum": 1, "maximum": 100}},
    ("query",))

KUMI_LIBRARY = _tool(
    "kumi_library",
    "The producer's sample library: its folders and scan state (status), add a folder Kumi may read (add_folder, which "
    "the producer approves), or scan the approved folders to build the sound map (scan, which the producer approves; it "
    "runs as a job: follow it with kumi_job). cancel stops the scan named by job_id: the reply only acknowledges the "
    "request, and kumi_job says cancelled once the scan has stopped. Kumi reads only folders added here.",
    {"action": {"type": "string", "enum": ["status", "add_folder", "scan", "cancel"]},
     "folder": {"type": "string", "description": "add_folder: an absolute folder path."},
     "job_id": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$",
                "description": "cancel: the job_id the scan returned."}},
    ("action",))

KUMI_JOB = _tool(
    "kumi_job",
    "The state, progress and result of a long Kumi job (a library scan, a Mirelo generation), by the job id the tool "
    "that started it returned. Reads only: it never creates or pays for anything.",
    {"job_id": {"type": "string"}}, ("job_id",))

KUMI_GENERATE = _tool(
    "kumi_generate",
    "Paid generation with Mirelo, only when the producer switched it on in Kumi's settings: a sound effect from a "
    "prompt, or MIDI notes from an audio file. First `quote` (kind sfx with prompt and optional seconds, or kind "
    "audio_to_midi with a source; sending that file to Mirelo is approved first). Then `create` with the quote_id: the "
    "producer approves it from the quote itself, with its estimate in credits. Credits are Mirelo API credits: an "
    "estimate and a reservation, never a price. The generation runs as a job: follow it with kumi_job. Never create a "
    "quote again after an unsure answer.",
    {"action": {"type": "string", "enum": ["quote", "create"]},
     "kind": {"type": "string", "enum": ["sfx", "audio_to_midi"], "description": "quote: what to generate."},
     "prompt": {"type": "string", "description": "quote, sfx: what the sound effect should be."},
     "seconds": {"type": "number", "description": "quote, sfx: how long, in seconds (5 when left out)."},
     "source": {"type": "object", "description": (
         "quote, audio_to_midi: the audio, as a source, never a path: {\"artifact_id\"} from a Kumi result, "
         "{\"sound_id\", \"map_revision\"} from kumi_sounds, or {\"file\", \"provenance\": {\"host_tool\", "
         "\"tool_call_id\"}} for a file another Hermes tool made in an approved folder.")},
     "quote_id": {"type": "string", "description": "create: the quote_id a quote returned."}},
    ("action",))
