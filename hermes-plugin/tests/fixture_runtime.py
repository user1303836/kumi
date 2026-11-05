"""A stand-in for `kumi serve --stdio` (RESEARCH.md C2, C3), for the plugin's tests and for building UIs without Kumi.

It takes the runtime's arguments, answers MCP over stdin and stdout as the runtime does, and holds a made-up Live Set
("Night Drive") in memory: connect, observe, read, a typed change with HISTORY and its preview (CC-3), undo, a small
sample library (two snares on map revision r1) with search, measuring and a scan job, a track or mix listen with its
preview (CC-2), setup's `setup_daw` (C8) on a made-up Live User Library, and the C3 snapshot files in `--state-dir`,
with a resource update after each. Standard library only.

    python fixture_runtime.py serve --stdio --state-dir DIR --kumi-home DIR --daw ableton [--library-root DIR]...
        [--server-version V] [--protocol P] [--exit-on TOOL] [--slow TOOL=SECONDS] [--leased-by SESSION]
        [--stderr-flood BYTES] [--close-stdout] [--ignore-eof]
        [--listen-outcome measured|refused|cancelled|unsure|unsure-with-media] [--listen-deadline SECONDS]
        [--component-state missing|same|older|newer|other_owner] [--component-owner DIR]

The knobs make it a runtime that's the wrong version, crashes or answers late on a tool, holds a lease for another chat,
floods its stderr, closes its output and stays, or stays after its stdin ends; and say how a track listen ends
(`unsure-with-media` is a runtime that wrongly sends a MEDIA: line with an unsure result).

It writes what it was started with (`fixture-start.json`: its arguments and environment's names) and every message it
received (`fixture-requests.jsonl`) into the state directory, for tests to read.
"""
from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import sys
import threading
import time
from typing import Any

VERSION = "1.24.66"
PROTOCOL = "kumi-serve/1"
MCP = "2025-11-25"
DAW_NAMES = {"ableton": "Live", "reaper": "REAPER", "renoise": "Renoise"}
# serve/change.rs's words for a change an earlier runtime made (agent-s, kumi-adv's D2).
RESTARTED = ("Kumi restarted since this change, so it can't undo it safely: undo it in {daw} if it's still the latest "
             "step there, or ask Hermes to set it back.")
RESOURCES = ("kumi://status", "kumi://focus", "kumi://history", "kumi://map")  # C2's, as #1047's kumi serve offers
# The plan steps the fixture applies, as the real runtime names Kumi's tools (W1-REGISTRY: `{tool, input, as}`), with
# the HISTORY family each one is in Live's change table (#965: a preview's `family`, so an approval's rule key).
STEPS = {"set_track_volume": "mixer", "replace_sample": "device"}
SET = {
    "name": "Night Drive", "key": "live:night-drive", "tempo": 124.0,
    "tracks": [
        {"ref": "t1", "name": "Drums", "volume_db": -6.0, "devices": [
            {"ref": "t1/d1", "name": "Drum Rack", "pads": [{"note": 38, "pad": "D1", "ref": "t1/d1/p38/d1",
                                                           "device": "Simpler", "sample": "Snare 01.wav"}]}]},
        {"ref": "t2", "name": "Bass", "volume_db": -8.0, "devices": [{"ref": "t2/d1", "name": "Operator"}]},
        {"ref": "t3", "name": "Keys", "volume_db": -10.0, "devices": [{"ref": "t3/d1", "name": "Electric"}]},
    ],
}


class Fixture:
    def __init__(self, options: argparse.Namespace):
        self.options = options
        self.state = Path(options.state_dir)
        self.state.mkdir(parents=True, exist_ok=True)
        self.artifacts: dict[str, dict] = {}  # artifacts.json's table, as W3's runtime keeps a measured file
        self.scans: dict[str, str] = {}  # job id -> running, cancelling, cancelled or done (CC-14)
        self.seq = 0
        self.connected = False
        self.daw = options.daw
        self.epoch = "1"
        self.set = copy.deepcopy(SET)
        # As kumi serve's History::new: an earlier runtime's rows stay for the record, none of them undoable from here.
        self.generation = options.generation
        kept = (json.loads((self.state / "history.json").read_text(encoding="utf-8"))
                if (self.state / "history.json").exists() else {})
        self.history: list[dict] = [{**row, "undo_available": False} for row in reversed(kept.get("items") or [])]
        self.owner: dict | None = None
        self.running = True
        self.since = now()
        self.seen_at: int | None = None  # the last look (connect or observe), as focus.json's project.seen_at
        self.subscribed: set[str] = set()  # the resources the host subscribed to (C2)
        self.map_revision: str | None = None
        self.slow = dict(item.split("=", 1) for item in options.slow)
        self.writing = threading.Lock()
        self.counting = threading.Lock()
        self.beating = threading.RLock()  # owner.json's writes: a heartbeat never lands after stopped()'s
        self.record("fixture-start.json", {"argv": sys.argv[1:], "environment": sorted(os.environ)})
        if options.stderr_flood:
            sys.stderr.write("noise " * (options.stderr_flood // 6) + "\n")
            sys.stderr.flush()

    # The wire.
    def send(self, message: dict) -> None:
        with self.writing:
            sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
            sys.stdout.flush()

    def serve(self) -> None:
        if self.options.publish_at_start:  # as #1047's kumi serve: a new runtime's HISTORY and FOCUS, before any call
            self.publish("history", "focus")
        for line in sys.stdin:
            message = json.loads(line)
            with open(self.state / "fixture-requests.jsonl", "a", encoding="utf-8") as log:
                log.write(json.dumps(message) + "\n")
            method, ident = message.get("method"), message.get("id")
            if ident is None:
                continue  # notifications: initialized, cancelled
            try:
                result = self.answer(method, message.get("params") or {})
            except LookupError as unknown:
                self.send({"jsonrpc": "2.0", "id": ident, "error": {"code": -32601, "message": str(unknown)}})
                continue
            self.send({"jsonrpc": "2.0", "id": ident, "result": result})
            if self.options.close_stdout and method == "initialize":
                sys.stdout.close()
                os.close(1)  # closes its output and goes on running, as a stuck runtime might
                time.sleep(3600)
        self.stopped()
        if self.options.ignore_eof:
            time.sleep(3600)  # stays after its stdin ends, as a runtime that never settles might

    def stopped(self) -> None:
        """At exit, as kumi serve's publish_stopped: status.json says stale and not connected, owner.json not running."""
        with self.beating:  # no heartbeat lands after this one
            self.running = False
            if self.owner is None:
                return
            self.record("status.json", {"schema": 1, "seq": self.next_seq(), "at": now(), "owner": self.owner,
                                        **self.status(), "stale": True, "connected": False})
            beat(self)

    def answer(self, method: str, params: dict) -> dict:
        if method == "initialize":
            offered = {} if self.options.no_resources else {"resources": {"subscribe": True, "listChanged": False}}
            return {"protocolVersion": MCP, "serverInfo": {"name": "kumi-serve", "version": self.options.server_version},
                    "capabilities": {"tools": {}, **offered}}
        if method == "tools/list":
            return {"tools": [{"name": name, "inputSchema": {"type": "object"}} for name in TOOLS]}
        if method == "tools/call":
            return self.call(params["name"], dict(params.get("arguments") or {}))
        if method in ("resources/subscribe", "resources/unsubscribe") and not self.options.no_resources:
            if params.get("uri") not in RESOURCES or params.get("uri") in self.options.refuse_subscribe:
                raise LookupError(f"no resource {params.get('uri')!r}")
            (self.subscribed.add if method == "resources/subscribe" else self.subscribed.discard)(params["uri"])
            return {}
        raise LookupError(f"no method {method}")

    def call(self, name: str, args: dict) -> dict:
        host = args.pop("_host", {})
        if name == self.options.exit_on:
            os._exit(3)
        if name in self.slow:
            time.sleep(float(self.slow[name]))
        if name not in TOOLS:
            return error("unknown_tool", f"kumi serve has no tool {name}")
        try:
            value = TOOLS[name](self, args, host)
        except (KeyError, TypeError, ValueError, AttributeError) as invalid:  # an OpError::Invalid, as the runtime's
            value = error("invalid", f"{name}'s arguments don't fit its schema: {invalid!r}")
        if "error" in value:
            return {"isError": True, "content": [{"type": "text", "text": value["error"]["reason"]}],
                    "structuredContent": value}
        return {"content": [{"type": "text", "text": json.dumps(value)}], "structuredContent": value}

    # The snapshots (C3).
    def record(self, name: str, value: dict) -> None:
        path = self.state / name
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(f"{path.suffix}.{threading.get_ident()}.tmp")  # one a thread: the heartbeat's own
        temporary.write_text(json.dumps(value, indent=1), encoding="utf-8")
        os.replace(temporary, path)

    def next_seq(self) -> int:
        with self.counting:
            self.seq += 1
            return self.seq

    def publish(self, *names: str) -> None:
        for name in names:
            seq = self.next_seq()
            body = {"status": self.status, "focus": self.focus, "history": self.history_snapshot}[name]()
            self.record(f"{name}.json", {"schema": 1, "seq": seq, "at": now(), "owner": self.owner, **body})
            self.landed(f"kumi://{name}")

    def publish_map(self, revision: str) -> None:
        """A scan's map (C3), as kumi serve ends one: the revision's file, then its pointer flips to it."""
        self.record(f"map/{revision}.json", {
            "schema": 1, "seq": self.next_seq(), "at": now(), "revision": revision, "made": now(), "points": [],
            "groups": {}, "sounds": {sound["sound_id"]: {key: sound[key] for key in ("name", "kind", "class", "folder",
                                                                                      "duration")} for sound in SOUNDS}})
        pointer = self.state / "map" / "current"
        pointer.with_name("current.tmp").write_text(f"{revision}\n", encoding="utf-8")
        os.replace(pointer.with_name("current.tmp"), pointer)
        self.map_revision = revision
        self.landed("kumi://map")

    def landed(self, uri: str) -> None:
        """A resource's snapshot is on disk anew: a subscriber hears of it (notifications/resources/updated)."""
        if uri in self.subscribed and uri not in self.options.quiet:
            self.send({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}})

    def project(self) -> dict | None:
        return {"name": self.set["name"], "key": self.set["key"], "epoch": self.epoch} if self.connected else None

    def status(self) -> dict:
        return {"runtime": {"version": self.options.server_version, "protocol": self.options.protocol},
                "connected": self.connected, "stale": False, "daw": self.daw, "project": self.project(),
                "owner": self.owner, "capabilities": ["observe", "read", "change", "undo", "history"]}

    def focus(self) -> dict:
        """focus.json as kumi serve writes it (CC-7): `project` with its epoch as a number (W1's u64) and `seen_at`,
        `focus` a DawFocus, `transport` without a clock, and `catch_up` CatchUp entries ({set, lastSeenAt, lines, more,
        afterReconnect?}), camelCase."""
        drums = self.set["tracks"][0]
        project = self.project()
        return {"project": {**project, "epoch": int(project["epoch"]), "seen_at": self.seen_at} if project else None,
                "focus": {"track": {"name": drums["name"]}, "trackRef": drums["ref"], "device": "Drum Rack",
                          "view": "Arrangement"},
                "transport": {"playing": False, "tempo": self.set["tempo"]}, "catch_up": []}

    def history_snapshot(self) -> dict:
        """HISTORY, newest first, as C3 has it: each row without the Set the fixture keeps to undo it, and this
        runtime's generation (#1047), which an earlier runtime's rows don't have."""
        return {"generation": self.generation, "items": [{key: value for key, value in row.items() if key != "before"}
                                                         for row in reversed(self.history)][:200]}

    def track(self, ref: str) -> dict | None:
        return next((track for track in self.set["tracks"] if track["ref"] == ref), None)

    def simpler(self, ref: str) -> tuple[dict, dict] | None:
        """The track and the drum pad whose Simpler has device ref `ref` (the demo's replace_sample target)."""
        return next(((track, pad) for track in self.set["tracks"] for device in track["devices"]
                     for pad in device.get("pads", []) if pad["ref"] == ref), None)


def now() -> int:
    return int(time.time() * 1000)


def error(code: str, reason: str) -> dict:
    return {"error": {"code": code, "reason": reason}}


def status(fixture: Fixture, args: dict, host: dict) -> dict:
    return fixture.status()


def connect(fixture: Fixture, args: dict, host: dict) -> dict:
    if args.get("daw", fixture.daw) != fixture.daw:
        return error("other_daw", f"kumi serve was started for {fixture.daw}")
    first = not fixture.connected
    fixture.connected = True
    fixture.seen_at = now()
    fixture.owner = {"profile": host.get("profile_id"), "session_id": host.get("session_id"), "pid": os.getpid()}
    beat(fixture)
    if first:
        threading.Thread(target=heartbeat, args=(fixture,), daemon=True).start()
    fixture.publish("status", "focus", "history")
    return fixture.status()


def beat(fixture: Fixture) -> None:
    """owner.json as kumi serve writes it: who runs it, its heartbeat, and whether it's still running."""
    with fixture.beating:
        fixture.record("owner.json", {"schema": 1, "seq": fixture.next_seq(), "at": now(), "owner": fixture.owner,
                                      **fixture.owner, "since": fixture.since, "heartbeat": now(),
                                      "running": fixture.running, "lease": "free"})


def heartbeat(fixture: Fixture) -> None:
    """owner.json's heartbeat, every 2 s while the runtime runs (C3: one older than 10 s means stale)."""
    while fixture.running:
        time.sleep(2)
        with fixture.beating:
            if not fixture.running:
                return
            try:
                beat(fixture)
            except OSError:
                pass  # Windows: a reader has owner.json open, so it can't be replaced now; the next beat will


def observe(fixture: Fixture, args: dict, host: dict) -> dict:
    if not fixture.connected:
        return error("not_connected", "Kumi isn't connected to the DAW: connect first.")
    tracks = "; ".join(f"{track['name']} ({track['ref']}, {track['volume_db']:g} dB)" for track in fixture.set["tracks"])
    text = f"{fixture.set['name']}, {fixture.set['tempo']:g} BPM. Tracks: {tracks}. FOCUS: Drums > Drum Rack."
    fixture.seen_at = now()
    fixture.publish("focus")  # written on each look (CC-7)
    published = fixture.focus()
    return {"text": text[:int(args.get("max_chars") or 4000)], "project_key": fixture.set["key"], "epoch": fixture.epoch,
            "project": published["project"], "focus": published["focus"], "transport": published["transport"],
            "catch_up": [], "age_ms": 0,
            "refs": [{"label": track["name"], "kind": "track", "detail": f"{track['volume_db']:g} dB",
                      "reference": {"kind": "daw", "node": track["ref"]}} for track in fixture.set["tracks"]]}


def read(fixture: Fixture, args: dict, host: dict) -> dict:
    if not fixture.connected:
        return error("not_connected", "Kumi isn't connected to the DAW: connect first.")
    if args.get("action") == "schema":
        if args.get("tool") not in STEPS:
            return error("unsupported", f"{args.get('tool')} isn't a step Kumi takes here")
        return {"tool": args["tool"], "schema": {"type": "object"}}
    return {"rows": copy.deepcopy(fixture.set["tracks"]), "cursor": None, "epoch": fixture.epoch}


def plan_refusal(fixture: Fixture, args: dict) -> tuple[str, str] | None:
    """Why a change plan can't run, as (code, reason) (#965: not_connected, stale, unknown_step, invalid, not_allowed),
    or None."""
    if not fixture.connected:
        return "not_connected", "Kumi isn't connected to the DAW: connect first."
    if (args.get("expected_project_key"), str(args.get("expected_epoch"))) != (fixture.set["key"], fixture.epoch):
        return "stale", "The project or its epoch changed since it was read: read it again."
    steps = args.get("steps") or []
    unknown = [step.get("tool") for step in steps if step.get("tool") not in STEPS]
    if not steps or len(steps) > 20 or unknown:
        return "unknown_step", f"Kumi doesn't take these steps here: {unknown or 'none given'}"
    for step in steps:
        target = step.get("input") or {}
        if step["tool"] == "set_track_volume":
            if fixture.track(target.get("track_ref", "")) is None:
                return "invalid", f"no track {target.get('track_ref')!r} in the project"
            if not isinstance(target.get("volume_db"), (int, float)):
                return "invalid", "set_track_volume needs volume_db, a number"
        elif step["tool"] == "replace_sample":
            if fixture.simpler(target.get("deviceRef", "")) is None:
                return "invalid", f"no Simpler {target.get('deviceRef')!r} in the project"
            refusal = source_refusal(fixture, target.get("sample"))
            if refusal is not None:
                return refusal
    return None


def source_refusal(fixture: Fixture, source: Any) -> tuple[str, str] | None:
    """CC-11: a file step's file field holds a source, never a path."""
    if isinstance(source, str):
        return "not_allowed", "A file step takes a sound, an artifact or an approved file as its source, never a path."
    if not isinstance(source, dict):
        return "invalid", "replace_sample needs sample, a source"
    if "sound_id" in source:
        if source.get("map_revision") != "r1":
            return "stale", "That sound is from another map revision: search again."
        if source["sound_id"] not in {sound["sound_id"] for sound in SOUNDS}:
            return "invalid", f"no sound {source['sound_id']!r} on the map"
        return None
    if "artifact_id" in source:
        return None if source["artifact_id"] in ("a-1", "a-track-1") else ("invalid", "no such artifact")
    if "file" in source:
        roots = [*fixture.options.library_root, *fixture.options.media_root]
        inside = isinstance(source["file"], str) and any(
            os.path.commonpath([os.path.abspath(source["file"]), os.path.abspath(root)]) == os.path.abspath(root)
            for root in roots)
        return None if inside else ("not_allowed", "That file isn't in an approved folder.")
    return "invalid", "replace_sample needs sample, a source"


def source_name(source: dict) -> str:
    """The file a source names, by its name only (CC-11: the preview never shows a path)."""
    if "sound_id" in source:
        return next(sound["name"] for sound in SOUNDS if sound["sound_id"] == source["sound_id"])
    if "artifact_id" in source:
        return f"{source['artifact_id']}.wav"
    return os.path.basename(source["file"])


def describe(fixture: Fixture, step: dict) -> dict:
    """A step's preview (CC-3) as #1005's runtime writes it: the step, its values first, then ` — on ` and its targets
    by their current names, each quoted and cut at 40 characters; and the step's HISTORY family."""
    target = step["input"]
    if step["tool"] == "set_track_volume":
        name = fixture.track(target["track_ref"])["name"]
        text = f"Set track volume: volume_db {float(target['volume_db']):g} — on {quoted(name)}"
    else:
        track, pad = fixture.simpler(target["deviceRef"])
        name = f"{pad['device']} ({track['name']} > {track['devices'][0]['name']} pad {pad['pad']})"
        text = f"Replace sample: sample {quoted(source_name(target['sample']))} — on {quoted(name)}"
    return {"tool": step["tool"], "family": STEPS[step["tool"]], "effect": "write", "text": text}


def quoted(name: str) -> str:
    return json.dumps(name if len(name) <= 40 else name[:39] + "…", ensure_ascii=False)


def change(fixture: Fixture, args: dict, host: dict) -> dict:
    if fixture.options.leased_by and not args.get("preview"):
        owner = {"profile": "default", "session_id": fixture.options.leased_by, "pid": 4242, "since": now()}
        return {"error": {"code": "leased", "reason": "Another chat holds the DAW for changes.", "owner": owner}}
    refusal = plan_refusal(fixture, args)
    if refusal is not None:
        return {"state": "refused", "code": refusal[0], "reason": refusal[1]}
    if args.get("preview"):
        return {"state": "preview", "title": args.get("title"),
                "project": {"key": fixture.set["key"], "epoch": fixture.epoch},
                "steps": [describe(fixture, step) for step in args["steps"]]}
    steps = args["steps"]
    before = copy.deepcopy(fixture.set)
    for step in steps:
        target = step["input"]
        if step["tool"] == "set_track_volume":
            fixture.track(target["track_ref"])["volume_db"] = float(target["volume_db"])
        else:
            fixture.simpler(target["deviceRef"])[1]["sample"] = source_name(target["sample"])
    first = steps[0]["input"]
    track = fixture.track(first["track_ref"]) if "track_ref" in first else fixture.simpler(first["deviceRef"])[0]
    ident = f"h-{fixture.set['key']}-{fixture.generation}-c{len(fixture.history) + 1}"
    # As kumi serve's Row: C1's fields, plus its own (label, project_key, runtime_generation, note) and CC-5's
    # (tool_call_id, digest, call_outcome), which the plugin never passes on.
    fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}", "project_key": fixture.set["key"],
                            "runtime_generation": fixture.generation, "title": args.get("title") or "Kumi change",
                            "family": STEPS[steps[0]["tool"]], "at": now(), "state": "applied",
                            "undo_available": True, "track": track["name"], "note": "kept at /Users/someone/Set.als",
                            "tool_call_id": host.get("tool_call_id"), "digest": "sha256:" + "0" * 64,
                            "call_outcome": "applied", "before": before})
    fixture.publish("history", "focus")
    done = [{"step": index + 1, "tool": step["tool"]} for index, step in enumerate(steps)]
    return {"state": "applied", "history_ids": [ident],
            "steps": {"done": done, "refused": [], "held_back": [], "stopped": None, "skipped": 0},
            "summary": args.get("title") or "Applied.", "undo_available": True, "project_key": fixture.set["key"],
            "epoch": fixture.epoch}


def undo(fixture: Fixture, args: dict, host: dict) -> dict:
    row = next((row for row in fixture.history if row["id"] == args.get("history_id")), None)
    # Why it can't be undone from here, as serve/change.rs says it: CC-3's reason, C1's state and the words.
    why_not = (("restarted", "expired", RESTARTED) if row and row["runtime_generation"] != fixture.generation
               else ("expired", "expired", "It can't be undone any more.") if row and not row["undo_available"]
               else None)
    if args.get("preview"):
        if row is None:
            return {"state": "refused", "reason": "No such change in this runtime's HISTORY."}
        preview = {"state": "preview", "text": f"Undo {row['title']} on {row['track']}", "undo_available": not why_not}
        return {**preview, "reason": why_not[0]} if why_not else preview
    if row is None:
        return {"state": "refused", "reason": "No such change in this runtime's HISTORY."}
    if why_not:
        return {"state": why_not[1], "reason": why_not[2].format(daw=DAW_NAMES[fixture.daw])}
    fixture.set = row.pop("before")
    row.update(state="undone", undo_available=False)
    fixture.publish("history", "focus")
    return {"state": "undone", "reason": f"Undid {row['title']}."}


def history(fixture: Fixture, args: dict, host: dict) -> dict:
    return {"items": fixture.history_snapshot()["items"][:int(args.get("limit") or 20)]}


# Each with the file's path, which a map has (CC-4) and the plugin must never pass to the model.
SOUNDS = [{"sound_id": "s-r1-1", "name": "Dusty Snare 04.wav", "kind": "snare", "class": "drum", "folder": "Drums",
           "duration": 0.42, "path": "/Samples/Drums/Dusty Snare 04.wav"},
          {"sound_id": "s-r1-2", "name": "Bright Snare 11.wav", "kind": "snare", "class": "drum", "folder": "Drums",
           "duration": 0.38, "path": "/Samples/Drums/Bright Snare 11.wav"}]


def library(fixture: Fixture, args: dict, host: dict) -> dict:
    """The library tool. One scan runs at a time (the busy guard, held until it settles); cancel answers only for the
    scan running or still settling from a cancel, again on a repeat (CC-14's 14a), and `not_running` for any other."""
    action = args.get("action")
    if action == "add_folder":
        fixture.options.library_root.append(args["folder"])
    if action == "scan":  # a scan job, its file written as kumi serve writes jobs/<id>.json
        if any(state in ("running", "cancelling") for state in fixture.scans.values()):
            return error("busy", "a scan is already running")
        job_id = f"j-scan-{len(fixture.scans) + 1}"
        fixture.scans[job_id] = "running"
        fixture.record(f"jobs/{job_id}.json", {"schema": 1, "seq": fixture.seq + 1, "at": now(), "job_id": job_id,
                                               "kind": "scan", "state": "running", "progress": {"done": 0, "total": 2}})
        return {"job_id": job_id}
    if action == "cancel":
        job_id = args.get("job_id")
        if fixture.scans.get(job_id) not in ("running", "cancelling"):
            return error("not_running", f"no scan {job_id!r} is running")
        fixture.scans[job_id] = "cancelling"
        return {"job_id": job_id, "cancel_requested": True}
    return {"folders": list(fixture.options.library_root), "sounds": len(SOUNDS), "map_revision": "r1"}


def sounds(fixture: Fixture, args: dict, host: dict) -> dict:
    words = str(args.get("query") or "").lower().split()
    found = [sound for sound in SOUNDS if all(word in sound["name"].lower() for word in words)]
    return {"map_revision": "r1", "items": found[:int(args.get("limit") or 20)]}


def listen(fixture: Fixture, args: dict, host: dict) -> dict:
    source = args.get("source") or {}
    if "track" in source:
        return listen_track(fixture, args, host)
    if source.get("sound_id") not in {sound["sound_id"] for sound in SOUNDS}:
        return error("not_found", "Kumi can only measure a sound of the current map in the fixture.")
    return {"metrics": analysis(lufs=-14.2, peak=-1.3, centroid=1840.0, seconds=0.42), "artifact_id": "a-1",
            "verdict_text": "Measured: -14.2 LUFS, true peak -1.3 dBTP, centroid 1840 Hz.",
            "media_tag": kept(fixture, "a-1")}


# A short silent WAV (44-byte header, 4 samples), standing in for a measured file.
WAV = (b"RIFF" + (44 - 8 + 8).to_bytes(4, "little") + b"WAVEfmt " + (16).to_bytes(4, "little")
       + (1).to_bytes(2, "little") + (1).to_bytes(2, "little") + (48000).to_bytes(4, "little")
       + (96000).to_bytes(4, "little") + (2).to_bytes(2, "little") + (16).to_bytes(2, "little")
       + b"data" + (8).to_bytes(4, "little") + bytes(8))


def kept(fixture: Fixture, artifact_id: str) -> str:
    """A measured file kept as W3's runtime keeps one: `<state>/artifacts/<id>.wav`, listed in artifacts.json. Its
    `MEDIA:` line."""
    path = fixture.state / "artifacts" / f"{artifact_id}.wav"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(WAV)
    fixture.artifacts[artifact_id] = {"path": str(path), "kind": "audio", "mime": "audio/wav", "tool": "listen",
                                      "at": now()}
    fixture.record("artifacts.json", {"schema": 1, "seq": fixture.next_seq(), "at": now(), "owner": fixture.owner,
                                      "artifacts": dict(fixture.artifacts)})
    return f"MEDIA:{path}"


def analysis(*, lufs: float, peak: float, centroid: float, seconds: float) -> dict:
    """W3's measurements (`kumi_media::audio::Analysis`, camelCase), as kumi serve's listen returns them."""
    return {"kumiAudio": 1, "format": "WAV", "sampleRate": 48000, "channels": 2, "seconds": seconds,
            "analyzed": {"from": "0:00", "to": "0:00", "focus": "mix"},
            "loudness": {"integratedLufs": lufs, "shortTermMaxLufs": lufs + 1.5, "rangeLu": 2.0, "truePeakDbtp": peak,
                         "samplePeakDbfs": peak - 0.3, "clippedSamples": 0},
            "balance": {"centroidHz": centroid, "tiltDbPerOctave": -1.2,
                        "bands": [{"name": "bass", "hz": "60–120", "db": -9.0, "width": 0.1, "correlation": 0.8},
                                  {"name": "presence", "hz": "2k–4k", "db": -14.0, "width": 0.2, "correlation": 0.7}]},
            "stereo": {"correlation": 0.7, "width": 0.2, "lowEndMono": True},
            "dynamics": {"crestDb": 9.0, "peakToLoudnessDb": 13.0, "onsetsPerSecond": 2.0},
            "tempo": None, "key": None,
            "spectrogram": {"rows": [{"band": "presence", "cells": "96·"}, {"band": "bass", "cells": "·39"}],
                            "columns": "0:00 to 0:00, 3 steps", "scale": "9 = loudest, each step 3 dB quieter, · = silent"}}


def listen_track(fixture: Fixture, args: dict, host: dict) -> dict:
    """A track or the mix over a beat window (CC-2). Its preview is a read; a real one "plays" the window and measures
    it, or ends as --listen-outcome says. Each real one is added to fixture-listens.jsonl."""
    if not fixture.connected:
        return error("not_connected", "Kumi isn't connected to the DAW: connect first.")
    if args.get("expected_project_key") != fixture.set["key"] or args.get("expected_epoch") != fixture.epoch:
        return error("stale", "The project changed since Kumi last looked: observe it again.")
    source = args["source"]
    track = source.get("track") or {}
    named = next((each for each in fixture.set["tracks"] if each["ref"] == track.get("ref")), None)
    if not track.get("mix") and named is None:
        return error("stale", f"There's no track {track.get('ref')!r} in {fixture.set['name']} now: observe it again.")
    start, beats = source["from_beat"], source["beats"]
    seconds = beats * 60 / fixture.set["tempo"]
    if seconds > 60:
        return error("too_long", f"{beats:g} beats at {fixture.set['tempo']:g} BPM is {seconds:.0f} s; Kumi listens to "
                                 "60 s at most.")
    what = "the mix" if track.get("mix") else f"the track {named['name']}"
    text = (f"play {what} of {fixture.set['name']} from beat {start:g} to beat {start + beats:g} (Arrangement) "
            "through Kumi Ears and measure it; routing and the transport change for a moment and are put back")
    if args.get("preview"):
        return {"state": "preview", "project": {"key": fixture.set["key"], "epoch": fixture.epoch}, "text": text,
                "estimated_seconds": round(seconds, 2), "method": "ears",
                "deadline_seconds": fixture.options.listen_deadline}
    with open(fixture.state / "fixture-listens.jsonl", "a", encoding="utf-8") as log:
        log.write(json.dumps({"args": args, "host": host}) + "\n")
    measured = {"metrics": analysis(lufs=-11.8, peak=-0.6, centroid=2210.0, seconds=round(seconds, 2)),
                "verdict_text": "Measured: -11.8 LUFS, true peak -0.6 dBTP, centroid 2210 Hz."}
    outcome = fixture.options.listen_outcome
    if outcome == "refused":
        return {"state": "refused", "code": "playing", "reason": "Live started playing before Kumi could listen.",
                "cleanup": "restored"}
    if outcome == "cancelled":
        return {"state": "cancelled", "code": "cancelled", "reason": "The listen was cancelled.", "cleanup": "restored"}
    if outcome in ("unsure", "unsure-with-media"):
        fixture.epoch = str(int(fixture.epoch) + 1)  # an unsure call moves the project on (CC-2's W1-c)
        fixture.publish("status", "focus")
        unsure = {"state": "unsure", "code": "restore_failed", "cleanup": "unsure",
                  "reason": "Kumi couldn't confirm Live's routing was put back."}
        if outcome == "unsure-with-media":
            unsure.update(measured, artifact_id="a-partial", media_tag=kept(fixture, "a-partial"))
        return unsure
    return {**measured, "artifact_id": "a-track-1", "media_tag": kept(fixture, "a-track-1"),
            "capture": {"method": "ears", "from_beat": start, "beats": beats, "seconds": round(seconds, 2),
                        "cleanup": "restored"}}


def job(fixture: Fixture, args: dict, host: dict) -> dict:
    """A scan's job. It settles when it's next read: a cancelled one as cancelled, any other as done, its map then the
    current one."""
    job_id = args.get("job_id")
    state = fixture.scans.get(job_id)
    if state is None:
        return error("not_found", f"no job {job_id!r}")
    fixture.scans[job_id] = "cancelled" if state in ("cancelling", "cancelled") else "done"
    if fixture.scans[job_id] == "cancelled":
        return {"state": "cancelled"}
    if fixture.map_revision != "r1":
        fixture.publish_map("r1")
    return {"state": "done", "progress": 1.0, "result": {"sounds": len(SOUNDS), "map_revision": "r1"}}


def setup_daw(fixture: Fixture, args: dict, host: dict) -> dict:
    """The runtime's setup_daw (C8, W1-REGISTRY): a dry run names the exact paths and writes nothing; a real install
    of a missing or older component "installs" it (recorded in the state directory, nothing else written)."""
    if args.get("daw") != fixture.daw:
        return error("other_daw", f"kumi serve was started for {fixture.daw}")
    state, owner = fixture.options.component_state, fixture.options.component_owner
    library = Path(fixture.options.state_dir).resolve() / "fake-user-library"
    script, ears = library / "Remote Scripts" / "AbletonMcpBridge", library / "Kumi" / "Audio Effects" / "Kumi Ears.amxd"
    reply = {"daw": fixture.daw, "component": "Live Remote Script", "version": "1.0.105",
             "paths": [str(script), str(ears)], "state": state,
             "owner": {"kumi_home": owner} if owner else None, "after": "choose_control_surface",
             "effects": [f"Copy the AbletonMcpBridge Remote Script into {script.parent}",
                         f"Copy the Kumi Ears device to {ears} (track listening needs Max for Live)"],
             "parts": [{"name": "Remote Script", "path": str(script), "state": state},
                       {"name": "Kumi Ears", "path": str(ears), "state": state}]}
    if args.get("dry_run"):
        return reply
    if state == "other_owner":
        return error("owned_elsewhere", "Another Kumi installation owns Live's component.")
    if state == "newer":
        return error("newer_installed", "Live has a newer Kumi component than this runtime.")
    fixture.options.component_state = "same"
    fixture.record("fixture-setup-daw.json", {"installed": reply["paths"], "host": host})
    return {**reply, "state": "same", "installed_at": now(),
            "parts": [{**part, "state": "same"} for part in reply["parts"]]}


TOOLS = {"status": status, "connect": connect, "observe": observe, "read": read, "change": change, "undo": undo,
         "history": history, "library": library, "sounds": sounds, "listen": listen, "job": job,
         "setup_daw": setup_daw}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["serve"])
    parser.add_argument("--stdio", action="store_true", required=True)
    parser.add_argument("--state-dir", required=True)
    parser.add_argument("--kumi-home", required=True)
    parser.add_argument("--daw", choices=["ableton", "reaper", "renoise"], required=True)
    parser.add_argument("--server-version", default=VERSION)
    parser.add_argument("--protocol", default=PROTOCOL)
    parser.add_argument("--exit-on")
    parser.add_argument("--slow", action="append", default=[])
    parser.add_argument("--leased-by", help="refuse changes as another chat's lease would")
    parser.add_argument("--stderr-flood", type=int, default=0, help="bytes written to stderr on start")
    parser.add_argument("--close-stdout", action="store_true", help="close stdout after initialize, and stay")
    parser.add_argument("--ignore-eof", action="store_true", help="stay after stdin ends")
    parser.add_argument("--generation", default="g1", help="this runtime's generation, as HISTORY's ids carry it")
    parser.add_argument("--no-resources", action="store_true", help="offer no resources, as kumi serve before #1047")
    parser.add_argument("--refuse-subscribe", action="append", default=[], help="refuse a subscription to this URI")
    parser.add_argument("--quiet", action="append", default=[], help="never tell a subscriber this URI changed")
    parser.add_argument("--publish-at-start", action="store_true", help="write HISTORY and FOCUS as it starts")
    parser.add_argument("--library-root", action="append", default=[])
    parser.add_argument("--hermes-home", help="the profile's Hermes home, as kumi serve takes it (#985)")
    parser.add_argument("--hermes-root", help="the root holding every Hermes profile, as kumi serve takes it (#985)")
    parser.add_argument("--media-root", action="append", default=[])
    parser.add_argument("--listen-outcome", default="measured",
                        choices=["measured", "refused", "cancelled", "unsure", "unsure-with-media"])
    parser.add_argument("--listen-deadline", type=int, default=75,
                        help="the preview's deadline_seconds (whole seconds): the capture's 65 s cap, restoring and "
                             "measuring")
    parser.add_argument("--component-state", default="missing",
                        choices=["missing", "same", "older", "newer", "other_owner"])
    parser.add_argument("--component-owner")
    Fixture(parser.parse_args()).serve()


if __name__ == "__main__":
    main()
