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
        [--listen-prepare SECONDS] [--listen-estimated SECONDS]
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
import hashlib
import json
import os
from pathlib import Path
import re
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
STEPS = {"set_track_volume": "mixer", "replace_sample": "device", "edit_rack": "device"}
# CC-20's 20c, as serve words it on macOS: the approval's first line, a row's note and every refusal of its undo.
DAW_UNDO = "Kumi can't undo this change. Live's own Undo (Cmd+Z) takes all of it back while it's still Live's latest step."
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
        self.quoted: dict[str, dict] = {}  # Mirelo quotes this runtime made (CC-13)
        self.generations: dict[str, dict] = {}  # generation jobs it created
        self.ledger_busy_said = False  # --generate-create ledger_busy_once: whether its one busy answer was given
        self.previews: dict[tuple, str] = {}  # an upload preview's file identity, by the host call it was for (A1)
        self.bounces = 0  # file listens with compare_to so far: each mastering pass comes closer to its reference
        self.devices: set[str] = set()  # the .amxd names in the User Library's Kumi folder (CC-22)
        self.device_files: dict[str, str] = {}  # a preview's file, by its spec's SHA-256 (W1-B)
        self.preset_files: set[str] = set()  # the .vstpreset labels Kumi wrote in its User Library folders (CC-26)
        self.wavetables: set[str] = set()  # the wavetable names Kumi wrote (the plugin-parity contract)
        self.seq = 0
        self.connected = False
        self.playing = False  # whether transport's play started it (CC-17)
        self.daw = options.daw
        self.takes = 0  # the highest take number given (CC-24: numbering goes on, V3 after V2)
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
        self.record("fixture-start.json", {"argv": sys.argv[1:], "environment": sorted(os.environ),
                                           "pid": os.getpid()})  # its own: on Windows the client's is kumi.bat's cmd.exe
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
                log.write(json.dumps(logged(message)) + "\n")
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
            self.record("status.json", {**self.status(), "stale": True, "connected": False, "schema": 1,
                                        "seq": self.next_seq(), "at": now(), "owner": self.owner})
            beat(self)

    def answer(self, method: str, params: dict) -> dict:
        if method == "initialize":
            offered = {} if self.options.no_resources else {"resources": {"subscribe": True, "listChanged": False}}
            return {"protocolVersion": MCP, "serverInfo": {"name": "kumi-serve", "version": self.options.server_version},
                    "capabilities": {"tools": {}, **offered}}
        if method == "tools/list":
            if self.options.no_tools_list:
                raise LookupError("no method tools/list")
            metadata = json.loads(self.options.listen_meta) if self.options.listen_meta is not None else None
            return {"tools": [{"name": name, "inputSchema": {"type": "object"},
                               **({"_meta": metadata} if name == "listen" and metadata is not None else {})}
                              for name in TOOLS]}
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
        replaced(temporary, path)

    def next_seq(self) -> int:
        with self.counting:
            self.seq += 1
            return self.seq

    def publish(self, *names: str) -> None:
        for name in names:
            seq = self.next_seq()
            body = {"status": self.status, "focus": self.focus, "history": self.history_snapshot}[name]()
            self.record(f"{name}.json", {**body, "schema": 1, "seq": seq, "at": now(), "owner": self.owner})
            self.landed(f"kumi://{name}")

    def publish_map(self, revision: str) -> None:
        """A scan's map (C3), as kumi serve ends one: the revision's file, then its pointer flips to it."""
        self.record(f"map/{revision}.json", {
            "schema": 1, "seq": self.next_seq(), "at": now(), "revision": revision, "made": now(), "points": [],
            "groups": {}, "sounds": {sound["sound_id"]: {key: sound[key] for key in ("name", "kind", "class", "folder",
                                                                                      "duration")} for sound in SOUNDS}})
        pointer = self.state / "map" / "current"
        pointer.with_name("current.tmp").write_text(f"{revision}\n", encoding="utf-8")
        replaced(pointer.with_name("current.tmp"), pointer)
        self.map_revision = revision
        self.landed("kumi://map")

    def landed(self, uri: str) -> None:
        """A resource's snapshot is on disk anew: a subscriber hears of it (notifications/resources/updated)."""
        if uri in self.subscribed and uri not in self.options.quiet:
            self.send({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}})

    def project(self) -> dict | None:
        return {"name": self.set["name"], "key": self.set["key"], "epoch": self.epoch} if self.connected else None

    def status(self) -> dict:
        """The status tool's answer, as kumi serve's (serve/core.rs): `owner` is the DAW's write lease while a call
        holds it (pid and profile, never a chat), so None between this fixture's calls. A snapshot's envelope names
        the chat (`owner`, set after the body, as serve/snapshots.rs does)."""
        return {"runtime": {"version": self.options.server_version, "protocol": self.options.protocol},
                "connected": self.connected, "stale": False, "daw": self.daw, "project": self.project(),
                "owner": None, "capabilities": ["observe", "read", "change", "undo", "history"]}

    def focus(self) -> dict:
        """focus.json as kumi serve writes it (CC-7): `project` with its epoch as a number (W1's u64) and `seen_at`,
        `focus` a DawFocus, `transport` without a clock, and `catch_up` CatchUp entries ({set, lastSeenAt, lines, more,
        afterReconnect?}), camelCase."""
        drums = self.set["tracks"][0]
        project = self.project()
        return {"project": {**project, "epoch": int(project["epoch"]), "seen_at": self.seen_at} if project else None,
                "focus": {"track": {"name": drums["name"]}, "trackRef": drums["ref"], "device": "Drum Rack",
                          "deviceRef": drums["devices"][0]["ref"], "view": "Arrangement"},
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

    def rack(self, ref: str) -> tuple[dict, dict] | None:
        """The track and the rack device with ref `ref` (CC-20's add-chain target)."""
        return next(((track, device) for track in self.set["tracks"] for device in track["devices"]
                     if device["ref"] == ref and device["name"].endswith("Rack")), None)


def now() -> int:
    return int(time.time() * 1000)


# As kumi serve replaces a snapshot (serve/snapshots.rs `replace`, TRIES): Windows refuses to rename over a file a
# reader has open, such as the plugin's snapshot watcher reading it just then, so the writer tries again, 20 ms apart.
# Without this, a change's publish raised PermissionError on Windows and ended this runtime's loop mid-test (#1141).
REPLACE_TRIES = 25


def replaced(temporary: Path, path: Path) -> None:
    for attempt in range(REPLACE_TRIES):
        try:
            os.replace(temporary, path)
            return
        except PermissionError:
            if attempt == REPLACE_TRIES - 1:
                raise
            time.sleep(0.02)


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
    return {"text": text[:int(args.get("max_chars") or 12000)], "project_key": fixture.set["key"], "epoch": fixture.epoch,
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
    kinds = {step.get("tool") == "edit_rack" for step in steps}
    if len(kinds) > 1:  # as #1137 refuses: Live's own Undo would take a mixed change back in parts
        return "not_allowed", ("A step only Live's own Undo takes back (edit_rack's add-chain) goes in a change of its "
                               "own, so that Live's Undo takes the whole change back.")
    for step in steps:
        target = step.get("input") or {}
        if step["tool"] == "set_track_volume":
            if fixture.track(target.get("track_ref", "")) is None:
                return "invalid", f"no track {target.get('track_ref')!r} in the project"
            if not isinstance(target.get("volume_db"), (int, float)):
                return "invalid", "set_track_volume needs volume_db, a number"
        elif step["tool"] == "edit_rack":
            if target.get("action") != "add-chain" or fixture.rack(target.get("deviceRef", "")) is None:
                return "invalid", "edit_rack here adds a chain to a rack in the project"
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
    elif step["tool"] == "edit_rack":
        track, rack = fixture.rack(target["deviceRef"])
        name = f"{rack['name']} ({track['name']})"
        text = f"Edit rack: action add-chain — on {quoted(name)}"
        return {"tool": step["tool"], "family": STEPS[step["tool"]], "effect": "write", "text": text, "undo": "daw"}
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
        described = [describe(fixture, step) for step in args["steps"]]
        daw_undo = {"undo": "daw", "undo_note": DAW_UNDO} if any(step.get("undo") for step in described) else {}
        return {"state": "preview", "title": args.get("title"),
                "project": {"key": fixture.set["key"], "epoch": fixture.epoch}, "steps": described, **daw_undo}
    steps = args["steps"]

    first = steps[0]["input"]
    track = (fixture.track(first["track_ref"]) if "track_ref" in first
             else fixture.rack(first["deviceRef"])[0] if steps[0]["tool"] == "edit_rack"
             else fixture.simpler(first["deviceRef"])[0])
    # As kumi serve records a change: a HISTORY row per family its steps touch, in plan order, each with the state
    # before it, and one serve-made change_id on all of them (CC-3 amendment A2). A rack's add-chain row is one only
    # Live's own Undo takes back (CC-20), so Kumi's undo isn't offered on it.
    change_id = f"ch-{len(fixture.history) + 1}"
    # A Live plan of three steps or more keeps a copy of the Set beside it first, as serve's change answer and its rows
    # say it (#1205's `copy`: the file's name and why; absent when none was kept).
    kept = {"file": f"{fixture.set['name']}.backup-2026-11-06.als", "why": "third_step"} if len(steps) >= 3 else None
    ids = []
    for family in dict.fromkeys(STEPS[step["tool"]] for step in steps):
        before = copy.deepcopy(fixture.set)
        mine = [step for step in steps if STEPS[step["tool"]] == family]
        for step in mine:
            target = step["input"]
            if step["tool"] == "set_track_volume":
                fixture.track(target["track_ref"])["volume_db"] = float(target["volume_db"])
            elif step["tool"] == "edit_rack":
                fixture.rack(target["deviceRef"])[1]["chains"] = fixture.rack(target["deviceRef"])[1].get("chains", 1) + 1
            else:
                fixture.simpler(target["deviceRef"])[1]["sample"] = source_name(target["sample"])
        daw_undo = any(step["tool"] == "edit_rack" for step in mine)
        ident = f"h-{fixture.set['key']}-{fixture.generation}-c{len(fixture.history) + 1}"
        # As kumi serve's Row: C1's fields, plus its own (label, project_key, runtime_generation, note, change_id)
        # and CC-5's (tool_call_id, digest, call_outcome), which the plugin never passes on.
        fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}", "project_key": fixture.set["key"],
                                "runtime_generation": fixture.generation, "title": args.get("title") or "Kumi change",
                                "family": family, "at": now(), "state": "applied",
                                "undo_available": not daw_undo, "track": track["name"], "daw_undo": daw_undo,
                                "note": DAW_UNDO if daw_undo else "kept at /Users/someone/Set.als",
                                "tool_call_id": host.get("tool_call_id"), "digest": "sha256:" + "0" * 64,
                                "call_outcome": "applied", "before": before, "change_id": change_id,
                                **({"copy": kept} if kept else {})})
        ids.append(ident)
    fixture.publish("history", "focus")
    done = [{"step": index + 1, "tool": step["tool"]} for index, step in enumerate(steps)]
    return {"state": "applied", "history_ids": ids,
            "steps": {"done": done, "refused": [], "held_back": [], "stopped": None, "skipped": 0},
            "summary": args.get("title") or "Applied.", "undo_available": True, "project_key": fixture.set["key"],
            "epoch": fixture.epoch, **({"copy": kept} if kept else {})}


def why_not(fixture: Fixture, row: dict) -> tuple[str, str, str] | None:
    """Why a row can't be undone from here, as serve/change.rs says it: CC-3's reason, C1's state and the words."""
    if row["runtime_generation"] != fixture.generation:
        return "restarted", "expired", RESTARTED
    if not row["undo_available"] and row["state"] != "undone":
        return "expired", "expired", "It can't be undone any more."
    return None


def undo(fixture: Fixture, args: dict, host: dict) -> dict:
    row = next((row for row in fixture.history if row["id"] == args.get("history_id")), None)
    if row is None:
        return {"state": "refused", "reason": "No such change in this runtime's HISTORY."}
    # As serve orders it: a script's row first (CC-19: Kumi never undoes one), then CC-20's, then the whole change.
    if row.get("family") == "script":
        if args.get("preview"):
            return {"state": "preview", "text": f"Undo {row['title']}", "undo_available": False,
                    "reason": "not_undoable", "note": SCRIPT_UNDO.format(daw=DAW_NAMES[fixture.daw])}
        return {"state": "refused", "code": "not_undoable", "reason": SCRIPT_UNDO.format(daw=DAW_NAMES[fixture.daw])}
    if row.get("daw_undo"):  # CC-20's 20e: refused in 20c's words, before the DAW is asked anything, whole or not
        if args.get("preview"):
            return {"state": "preview", "text": f"Undo {row['title']} on {row['track']}", "undo_available": False,
                    "reason": "daw_undo", "note": DAW_UNDO}
        return {"state": "refused", "code": "daw_undo", "reason": DAW_UNDO}
    if args.get("whole_change") is True:
        return undo_whole(fixture, args, row)
    refused = why_not(fixture, row) if row["state"] != "undone" else ("expired", "expired", "It can't be undone any more.")
    if args.get("preview"):
        preview = {"state": "preview", "text": f"Undo {row['title']} on {row['track']}", "undo_available": not refused}
        return {**preview, "reason": refused[0]} if refused else preview
    if refused:
        return {"state": refused[1], "reason": refused[2].format(daw=DAW_NAMES[fixture.daw])}
    fixture.set = row.pop("before")
    row.update(state="undone", undo_available=False)
    fixture.publish("history", "focus")
    return {"state": "undone", "reason": f"Undid {row['title']}."}


def undo_whole(fixture: Fixture, args: dict, row: dict) -> dict:
    """CC-3 amendment A2: every row of that row's change, newest first. Its preview lists them with their families and
    a digest of the set; the real call needs that digest back (`expect_rows`), undoes under one lease, and stops at
    the first row it can't undo."""
    rows = [each for each in reversed(fixture.history)
            if each is row or (row.get("change_id") and each.get("change_id") == row["change_id"])]
    # A2 (3): lower-case hex SHA-256 of the lines "<history_id>:<state>", newest first, joined by \n, none trailing.
    digest = hashlib.sha256("\n".join(f"{each['id']}:{each['state']}" for each in rows).encode()).hexdigest()
    if args.get("preview"):
        listed = [{"history_id": each["id"], "title": each["title"], "family": each["family"], "state": each["state"],
                   "text": f"Undo {each['title']} ({each['family']}) on {each['track']}",
                   "undo_available": each["state"] == "undone" or why_not(fixture, each) is None} for each in rows]
        text = listed[0]["text"] if len(listed) == 1 else \
            f"Undo {len(listed)} steps: " + "; ".join(each["text"] for each in listed)
        pending = [each for each in listed if each["state"] != "undone"]
        return {"state": "preview", "rows": listed, "rows_digest": digest,
                "undo_available": bool(pending) and all(each["undo_available"] for each in pending), "text": text[:600]}
    if args.get("expect_rows") != digest:
        return error("stale", "That change's steps aren't the ones approved: preview it again.")
    answered, stopped = [], None
    for each in rows:
        if stopped is not None:
            answered.append({"history_id": each["id"], "state": "not_tried"})
        elif each["state"] == "undone":
            answered.append({"history_id": each["id"], "state": "undone"})
        elif (refused := why_not(fixture, each)) is not None:
            stopped = refused[1]
            answered.append({"history_id": each["id"], "state": refused[1], "reason": refused[2]})
        else:
            fixture.set = each.pop("before")
            each.update(state="undone", undo_available=False)
            answered.append({"history_id": each["id"], "state": "undone"})
    fixture.publish("history", "focus")
    done = [each for each in answered if each["state"] == "undone"]
    state = "undone" if stopped is None else ("partly_undone" if done else stopped)
    return {"state": state, "rows": answered}


def history(fixture: Fixture, args: dict, host: dict) -> dict:
    return {"items": fixture.history_snapshot()["items"][:int(args.get("limit") or 20)]}


# CC-19's script op, as the lead's interface in CONTRACT-CHANGES has serve check and answer it.
SCRIPT_LANGUAGE = {"ableton": "python", "reaper": "lua", "renoise": "lua"}
SCRIPT_UNDO = "Kumi can't undo a script: {daw}'s own Undo may, if its step is still the latest there."
SCRIPT_DENIED = {"lua": ("os.execute", "io.popen", "socket"),
                 "python": ("subprocess", "os.system", "socket", "urllib", "ctypes")}
SCRIPT_HIDDEN = ({*range(0x202A, 0x202F), *range(0x2066, 0x206A), 0x200E, 0x200F, 0x061C, 0x200B, 0x200C, 0x200D,
                  0x2060, 0xFEFF, 0x2028, 0x2029, 0x7F, *range(0x80, 0xA0)}
                 | ({*range(0x20)} - {0x09, 0x0A, 0x0D}))


def script_refusal(fixture: Fixture, args: dict) -> tuple[str, str] | None:
    code, language = args.get("code"), args.get("language")
    if not isinstance(code, str) or not code.strip():
        return "invalid", "The script has no code."
    if language != SCRIPT_LANGUAGE[fixture.daw]:
        return "invalid", (f"Live runs Python scripts, and REAPER and Renoise run Lua: this one is "
                           f"{'Lua' if language == 'lua' else 'Python'}.")
    lines = len(code[:-1].split("\n")) if code.endswith("\n") else len(code.split("\n"))
    if lines > 400 or len(code.encode("utf-8")) > 32 * 1024:
        return "too_long", "The script is over 400 lines or 32 KB, so Kumi won't run it: split it, or use a typed step."
    for number, line in enumerate(code.split("\n"), 1):
        hidden = next((char for char in line if ord(char) in SCRIPT_HIDDEN), None)
        if hidden is not None:
            return "hidden_characters", (f"The script has a hidden or direction-changing character (U+{ord(hidden):04X} "
                                         f"on line {number}), which can make code read differently than it runs, so "
                                         "Kumi won't run it.")
    for entry in SCRIPT_DENIED[language]:
        if re.search(rf"(?<![\w.]){re.escape(entry)}(?![\w])", code):
            return "not_allowed", (f"Kumi refuses scripts that use `{entry}`: it reaches outside the DAW. (This check is "
                                   "a safeguard, not a guarantee.)")
    if not fixture.connected:
        return "not_connected", "Kumi isn't connected to the DAW: connect first."
    if args.get("expected_project_key") != fixture.set["key"]:
        return "stale", "The project changed since it was read: read it again."
    return None


def script(fixture: Fixture, args: dict, host: dict) -> dict:
    refusal = script_refusal(fixture, args)
    if refusal is not None:
        return {"state": "refused", "code": refusal[0], "reason": refusal[1]}
    code = args["code"]
    digest = hashlib.sha256(code.encode("utf-8")).hexdigest()
    if args.get("preview"):
        return {"state": "preview", "daw": fixture.daw, "language": args["language"], "purpose": args.get("purpose"),
                "sha256": digest, "lines": len(code.rstrip("\n").split("\n")), "bytes": len(code.encode("utf-8")),
                "deadline_seconds": 0.8 if fixture.daw == "renoise" else 10,
                "project": {"key": fixture.set["key"], "epoch": fixture.epoch}}
    if args.get("expected_sha256") != digest:
        return {"state": "refused", "code": "stale", "reason": "The script changed since it was approved."}
    ident = f"h-{fixture.set['key']}-{fixture.generation}-c{len(fixture.history) + 1}"
    fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}", "project_key": fixture.set["key"],
                            "runtime_generation": fixture.generation, "title": f"Script: {args.get('purpose')}",
                            "family": "script", "at": now(), "state": "applied", "undo_available": False,
                            "note": SCRIPT_UNDO.format(daw=DAW_NAMES[fixture.daw]),
                            "track": None, "script": {"sha256": digest, "language": args["language"]},
                            "tool_call_id": host.get("tool_call_id"), "call_outcome": "applied"})
    fixture.publish("history", "focus")
    return {"state": "ran", "history_id": ident, "history_ids": [ident],  # serve answers both
            "output": {"result": "ok", "stdout": "printed\n"},
            "project": {"key": fixture.set["key"], "epoch": fixture.epoch}}


# CC-22's device op, as the P9 interface has serve answer it: `spec_json` is hashed as received and parsed, never
# re-encoded; a preview is pure and names a free file; a build refuses another spec (`stale`) or a file that's no
# longer free (W1-B), and writes under a new name only.
DEVICE_FOLDER = "User Library/Kumi/Audio Effects"


def device_findings(spec: dict) -> list[str]:
    """A few of the builder's refusals (22b, 22-r1, 22-r2), each worded as `check_gen_code` words a finding."""
    code, found = spec.get("code") or "", []
    if re.search(r"\b(while|do)\b", code):
        found.append("code: `while` and `do` aren't allowed: a loop on Live's audio thread must end (a `for` with "
                     "literal bounds, at most 64).")
    if re.search(r"\bBuffer\b", code):
        found.append("code: Buffer reads a buffer~ anything in the Set can fill: use Data.")
    for control in spec.get("controls") or []:
        if control.get("type") in ("number", "integer") and not control.get("min", 0) < control.get("max", 1):
            found.append(f"{control.get('name')}: min isn't below max.")
    return found


def device(fixture: Fixture, args: dict, host: dict) -> dict:
    spec_json = args["spec_json"]
    digest = hashlib.sha256(spec_json.encode("utf-8")).hexdigest()
    spec = json.loads(spec_json)
    findings = device_findings(spec)
    if findings:
        return {"error": {"code": "refused", "reason": "Kumi won't build this device as it is: fix what's listed and "
                                                       "ask again.", "findings": findings}}
    free = next(name for name in (spec["name"], *(f"{spec['name']} {n}" for n in range(2, 100)))
                if name not in fixture.devices)
    file = f"{DEVICE_FOLDER}/{free}.amxd"
    code = spec["code"]
    lines = len(code[:-1].split("\n")) if code.endswith("\n") else len(code.split("\n"))
    if args["action"] == "preview":
        fixture.device_files[digest] = file
        shown = "0" * 64 if fixture.options.device_wrong_sha else digest
        return {"state": "preview", "name": free, "file": file, "controls": spec["controls"], "code": code,
                "lines": lines, "sha256": shown}
    if args["action"] != "build":
        return error("invalid", "action is preview or build")
    if args.get("sha256") != digest:
        return error("stale", "The device's spec changed since it was approved, so Kumi didn't build it.")
    if fixture.options.device_file_appears:  # a file with the previewed name appeared since
        fixture.devices.add(free)
    if fixture.device_files.get(digest) != f"{DEVICE_FOLDER}/{free}.amxd" or free in fixture.devices:
        return error("stale", "A file took the name the approval showed, so Kumi didn't build it: ask again.")
    fixture.devices.add(free)
    return {"name": free, "item": f"query:UserLibrary#Kumi:Audio%20Effects:{free}.amxd", "file": file,
            "artifact_id": f"a-device-{len(fixture.devices)}", "sha256": digest, "listed": True}


# CC-26's preset op, as agent-s's serve answers it (W2-CC26-PLUGIN.md), on one preset agent-m's writer built: a preview
# is pure and names a free file; a place writes exactly that file of exactly those bytes, or refuses `changed`.
PRESET_ARTIFACT = "a-preset-1"
PRESET_NOTE = "Values as the preset file stores them; the plug-in may show them differently."


def preset_record(options: argparse.Namespace) -> dict:
    """The writer's record of the fixture's one preset. By default a genuine receipt, as agent-m's native writer built
    it for Serum 2 2.1.5 (one edit, Oscillator1's volume). --preset-shapes swaps in a renderer-shape fixture, NOT a real
    receipt: each kind of value the record can hold in one list (a switch the template didn't store, a stored value
    with its range, a decimal exactly as stored under Pro-Q 4's catalog unit, a module chain). --preset-settings N makes
    N macro changes and --preset-unedited none, both shapes too."""
    settings = [{"path": "Oscillator1/plainParams/kParamVolume", "name": "B Level", "before": "not stored in template",
                 "after": 0.45, "unit": "stored value", "range": [0, 1]}]
    if options.preset_shapes:
        settings = [{"path": "osc/b/enable", "name": "Osc B", "before": "not stored in template", "after": True,
                     "unit": "stored value"},
                    {"path": "global/volume", "name": "Volume", "before": 0.3, "after": 0.45, "unit": "stored value",
                     "range": [0, 1]},
                    {"path": "band/1/gain", "name": "Gain", "before": 0, "after": 0.12345679104328156,
                     "unit": "stored VST3 plain value"},
                    {"path": "fx/order", "name": "FX Order", "before": ["Reverb"], "after": ["Distortion", "Reverb"],
                     "unit": "chain"}]
    if options.preset_settings:
        settings = [{"path": f"macro/{n}", "name": f"Macro {n}", "before": 0.0, "after": 0.5, "unit": "stored value"}
                    for n in range(1, options.preset_settings + 1)]
    if options.preset_unedited:
        settings = []
    name = "Night Bass\u202e" if options.preset_hidden else "Night Bass"
    return {"plugin": "serum-2", "class": "56534558667350736572756D20320000", "version": "2.1.5",
            "format": "vstpreset", "digest": hashlib.sha256(name.encode("utf-8")).hexdigest(), "name": name,
            "folder": "User Library/Kumi/Instruments", "settings": settings, "values_note": PRESET_NOTE}


def preset_build(fixture: Fixture, args: dict, host: dict) -> dict:
    """agent-m's preset_build: the verified writer's built preset, kept as an artifact, answered as W3's DTO."""
    if args.get("plugin") not in ("serum-2", "fabfilter-pro-q-4", "ozone-12"):
        return error("unsupported", "Use a verified Serum 2, Pro-Q 4 or Ozone 12 writer; Vital isn't supported.")
    if not isinstance(args.get("edits"), list) or len(args["edits"]) > 512:
        return error("invalid", "Use plugin, name and typed edits from the plug-in catalog; no file, bytes or version "
                                "override.")
    record = preset_record(fixture.options)
    return {"state": "built", "artifact_id": PRESET_ARTIFACT,
            **{key: record[key] for key in ("plugin", "class", "version", "format", "digest", "name", "settings",
                                            "values_note")}}


# The refusals serve's preset op gives that --preset-refusal names (#1180): the code and its words, with findings.
PRESET_REFUSALS = {
    "not_installed": ("Serum 2's VST3 isn't in Live's plug-ins. Install or enable it, then ask again.", None),
    "refused": ("Kumi won't write this preset as it is.",
                ["User Library/Kumi/Instruments isn't a plain folder (a link or a file is there)."]),
    "missing_artifact": ("The preset Kumi built is no longer in its store: build it again.", None),
}


def preset(fixture: Fixture, args: dict, host: dict) -> dict:
    """As #1180's preset op: the file's name is the preset's name and the first 12 hex of its digest, so the same
    bytes always name the same file, and placing them again answers that file without writing another."""
    if fixture.daw != "ableton":
        return error("unsupported", "Kumi puts plug-in presets only into Ableton Live's User Library.")
    if args.get("artifact_id") != PRESET_ARTIFACT:
        return error("invalid_source", "That artifact isn't a preset Kumi's verified writer made.")
    if fixture.options.preset_refusal:
        reason, findings = PRESET_REFUSALS[fixture.options.preset_refusal]
        refused = error(fixture.options.preset_refusal, reason)
        if findings:
            refused["error"]["findings"] = findings
        return refused
    record = preset_record(fixture.options)
    label = f"{record['name']} {record['digest'][:12]}"
    file = f"{record['folder']}/{label}.vstpreset"
    facts = {key: record[key] for key in ("plugin", "class", "version")}
    if args.get("action") == "preview":
        return {"state": "preview", **facts, "version_checked": not fixture.options.preset_unchecked,
                "format": record["format"], "digest": record["digest"], "settings": record["settings"], "file": file,
                "name": label, "values_note": record["values_note"]}
    if args.get("action") != "place" or not args.get("digest") or not args.get("file"):
        return error("invalid", "place takes the previewed digest and file.")
    if fixture.options.preset_changed or args["digest"] != record["digest"] or args["file"] != file:
        return error("changed", "The preset or its file changed since the preview, so Kumi wrote nothing: preview it "
                                "again.")
    fixture.preset_files.add(label)
    return {"name": label, "item": f"user_library/{file.removeprefix('User Library/')}", "file": file,
            "indexed": True, **facts, "next": "Observe, then load it with one kumi_change: load_device with this item."}


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
    """CC-23's search: the index's values and why each hit matched (the query's words, never a name's other text), and
    on a near search the seed apart and each hit's closeness. One reason here names a path, as a runtime must never:
    the plugin's second guard drops it."""
    words = str(args.get("query") or "").lower().split()
    found = [sound for sound in SOUNDS if all(word in sound["name"].lower() for word in words)
             and float(args.get("min_seconds") or 0) <= sound["duration"] <= float(args.get("max_seconds") or 1e9)
             and sound["sound_id"] != args.get("near")]
    hits = []
    for sound in found[:int(args.get("limit") or 20)]:
        why = [f"name matches '{word}'" for word in words][:3]
        if sound["sound_id"] == "s-r1-1":
            why.append("found in /Samples/Drums")
        hits.append({**sound, "bpm": None, "key": None, "note": "D2", "why": why,
                     **({"closeness": 87} if args.get("near") else {})})
    return {"map_revision": "r1", "indexed": True, **({"near": args["near"]} if args.get("near") else {}),
            "items": hits}


def listen(fixture: Fixture, args: dict, host: dict) -> dict:
    source = args.get("source") or {}
    if "track" in source:
        return listen_track(fixture, args, host)
    if "file" in source and isinstance(args.get("compare_to"), dict) and "file" in args["compare_to"]:
        return listen_bounce(fixture, source["file"], args["compare_to"]["file"])
    if source.get("sound_id") not in {sound["sound_id"] for sound in SOUNDS}:
        return error("not_found", "Kumi can only measure a sound of the current map in the fixture.")
    return {"metrics": analysis(lufs=-14.2, peak=-1.3, centroid=1840.0, seconds=0.42), "artifact_id": "a-1",
            "verdict_text": "Measured: -14.2 LUFS, true peak -1.3 dBTP, centroid 1840 Hz.",
            "media_tag": kept(fixture, "a-1")}


# A bounce of the mix after each mastering pass (flow A): integrated LUFS, tilt and stereo width, coming closer to the
# reference's -8.6 LUFS, -3.1 dB/oct and 0.31.
PASSES = ((-12.7, -3.9, 0.22), (-10.2, -3.4, 0.27), (-9.0, -3.2, 0.30))


def listen_bounce(fixture: Fixture, bounce: str, reference: str) -> dict:
    """A bounce measured against a reference file, as kumi serve's `retain` answers: the bounce's measurements, its
    audio and the reference's kept as two new artifacts, and the differences (current minus reference)."""
    fixture.bounces += 1
    lufs, tilt, width = PASSES[min(fixture.bounces, len(PASSES)) - 1]
    mine = analysis(lufs=lufs, peak=-1.0, centroid=2050.0 + 120 * fixture.bounces, seconds=30.0)
    mine["file"] = Path(bounce).name
    mine["balance"]["tiltDbPerOctave"] = tilt
    mine["balance"]["bands"] = [{**band, "db": band["db"] - 1.2 / fixture.bounces} for band in mine["balance"]["bands"]]
    mine["stereo"]["width"] = width
    theirs = analysis(lufs=-8.6, peak=-0.2, centroid=2450.0, seconds=30.0)
    theirs["file"] = Path(reference).name
    theirs["balance"]["tiltDbPerOctave"] = -3.1
    theirs["stereo"] = {"correlation": 0.5, "width": 0.31, "lowEndMono": True}
    theirs["dynamics"]["peakToLoudnessDb"] = 8.4
    difference = round(lufs + 8.6, 1)
    bands = [{"band": ours["name"], "hz": ours["hz"], "difference": round(ours["db"] - other["db"], 1)}
             for ours, other in zip(mine["balance"]["bands"], theirs["balance"]["bands"])]
    headlines = [f"{abs(difference)} LU quieter overall ({lufs} vs -8.6 LUFS)"]
    mine_id, reference_id = f"a-bounce-{fixture.bounces}", f"a-reference-{fixture.bounces}"
    kept(fixture, reference_id)
    return {"metrics": mine, "artifact_id": mine_id, "media_tag": kept(fixture, mine_id),
            "verdict_text": "\n".join([f"{lufs} LUFS", *headlines]),
            "compare": {"artifact_id": reference_id, "metrics": theirs,
                        "differences": {"headlines": headlines, "loudness": {"differenceLu": difference},
                                        "balance": bands}}}


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
        preview = {"state": "preview", "project": {"key": fixture.set["key"], "epoch": fixture.epoch}, "text": text,
                   "method": "ears"}
        for name, given, default in (("estimated_seconds", fixture.options.listen_estimated, round(seconds, 2)),
                                     ("prepare_seconds", fixture.options.listen_prepare,
                                      120 if args.get("compare_to") else 0),
                                     ("deadline_seconds", fixture.options.listen_deadline, 75)):
            if given != "missing":  # a runtime that doesn't give this bound at all
                preview[name] = default if given is None else json.loads(given)
        return preview
    bounds = args.get("_listen")
    if not isinstance(host, dict) or not host or not isinstance(bounds, dict) or sorted(bounds) != [
            "deadline_seconds", "estimated_seconds", "prepare_seconds"]:
        return error("invalid", "_listen")  # the runtime's own refusal (CC-2's A2): only the host's approved bounds play
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
    """A generation's job, or a scan's: a scan settles when it's next read, a cancelled one as cancelled, any other as
    done, its map then the current one."""
    job_id = args.get("job_id")
    if job_id in fixture.generations:
        return generation_job(fixture, job_id)
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


def logged(message: dict) -> dict:
    """What the fixture keeps of a request: a `_mirelo` key is never written (kumi serve strips it from every record,
    13-R3); that one came, and the request's other `_mirelo` fields, are."""
    arguments = (message.get("params") or {}).get("arguments")
    if not isinstance(arguments, dict) or not isinstance(arguments.get("_mirelo"), dict):
        return message
    sealed = {key: value for key, value in arguments["_mirelo"].items() if key != "api_key"}
    sealed["api_key"] = "<given>" if arguments["_mirelo"].get("api_key") else None
    return {**message, "params": {**message["params"], "arguments": {**arguments, "_mirelo": sealed}}}


def generate(fixture: Fixture, args: dict, host: dict) -> dict:
    """Mirelo generation as W3's native `generate` answers it (CC-13): a quote (for MIDI, first a preview naming the
    file it would send, pinned to the host call it was for, and that call's quote is stale without it), a create that
    needs the host's `_mirelo` with a reservation, and its job."""
    action, sealed = args.get("action"), args.get("_mirelo") if isinstance(args.get("_mirelo"), dict) else {}
    if action == "quote":
        kind, source = args.get("kind"), args.get("source") if isinstance(args.get("source"), dict) else {}
        name = next((sound["name"] for sound in SOUNDS if sound["sound_id"] == source.get("sound_id")), "a-1.wav")
        call = (host.get("profile_id"), host.get("session_id"), host.get("tool_call_id"))
        identity = hashlib.sha256(json.dumps(source, sort_keys=True).encode()).hexdigest()
        if kind == "audio_to_midi" and args.get("preview"):
            if call[2] is not None:
                fixture.previews[call] = identity
            return {"state": "preview", "upload": {"file": name, "bytes": 480044, "identity_sha256": identity}}
        if not sealed.get("api_key"):
            return error("not_configured", "no Mirelo key came with this quote")
        if kind == "audio_to_midi" and fixture.previews.pop(call, None) != identity:
            return error("stale", "no upload preview was pinned for this call and file")
        sfx = kind == "sfx"
        seconds = float(args.get("seconds") or 5) if sfx else 0.42
        request = {"kind": kind, "prompt": args.get("prompt"), "seconds": seconds, "source": source}
        quote_id = f"q-{os.getpid()}-{len(fixture.quoted) + 1}"  # unique across restarts, as the runtime's are
        credits = fixture.options.generate_estimate
        fixture.quoted[quote_id] = quote = {
            "state": "quoted", "quote_id": quote_id, "kind": kind, "prompt": args.get("prompt") if sfx else None,
            "file": None if sfx else name, "seconds": seconds, "estimate": {"credits": credits, "max_credits": credits},
            "credit_unit": "mirelo_credits", "upload": not sfx,
            "expires_at": int(time.time() * 1000) + fixture.options.generate_expires_ms,
            "request_sha256": hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest(),
            "summary": f"One {seconds:g}-second sound effect" if sfx else f"MIDI notes from {name}"}
        return quote
    if action == "create":
        quote = fixture.quoted.get(args.get("quote_id"))
        if quote is None:
            return error("unknown_quote", "no such quote")
        if not sealed.get("api_key") or not sealed.get("reservation_id"):
            return error("not_approved", "a create needs the host's reservation")
        if fixture.options.generate_create == "not_submitted":
            return error("not_submitted", "Mirelo refused it before anything was sent")
        busy = fixture.options.generate_create == "ledger_busy" or (
            fixture.options.generate_create == "ledger_busy_once" and not fixture.ledger_busy_said)
        if busy:  # its ledger lock timed out before anything was posted; the same call may come again
            fixture.ledger_busy_said = True
            return error("ledger_unavailable", "Kumi's credit ledger is busy.")
        job_id = f"g-{os.getpid()}-{len(fixture.generations) + 1}"
        fixture.generations[job_id] = {"quote": quote, "reserved": sealed.get("reserved_credits")}
        fixture.record(f"jobs/{job_id}.json", {"schema": 1, "seq": fixture.next_seq(), "at": now(), "job_id": job_id,
                                               "kind": "generate", "state": "running"})  # as kumi serve lists its jobs
        return {"state": "running", "job_id": job_id, "quote_id": quote["quote_id"], "kind": quote["kind"],
                "prompt": quote["prompt"], "seconds": quote["seconds"], "estimate": quote["estimate"],
                "credit_unit": "mirelo_credits"}
    if action == "job":
        return generation_job(fixture, args.get("job_id"))
    return error("invalid_request", "action is quote, create or job")


def generation_job(fixture: Fixture, job_id: object) -> dict:
    """A generation job's envelope, as `generate`'s job action and the plain `job` tool both give it (C3's one job
    store): its checkpointed result names its quote, whatever became of the create's answer."""
    made = fixture.generations.get(job_id)
    if made is None:
        return error("not_found", f"no job {job_id!r}")
    quote, artifact = made["quote"], f"a-{job_id}"
    actual = fixture.options.generate_actual
    tag = kept(fixture, artifact)
    return {"job_id": job_id, "kind": "generate", "state": "done", "progress": {"done": 1, "total": 1},
            "result": {"quote_id": quote["quote_id"], "kind": quote["kind"], "prompt": quote["prompt"],
                       "seconds": quote["seconds"], "provider": "mirelo", "provider_job_id": "v-1",
                       "submission": "submitted", "artifact_id": artifact, "file": f"{artifact}.wav",
                       "output_type": "audio", "media_tag": tag,
                       "outputs": [{"artifact_id": artifact, "file": f"{artifact}.wav", "type": "audio",
                                    "mime": "audio/wav", "bytes": len(WAV)}],
                       "credits": {"estimate": quote["estimate"]["credits"], "reserved": made["reserved"],
                                   "actual": None if actual < 0 else actual},
                       "credit_unit": "mirelo_credits"}}


def transport(fixture: Fixture, args: dict, host: dict) -> dict:
    """Playback as serve's `transport` (CC-17, #1143): a pure preview in serve's words; play refused while the DAW
    records (the fixture's --recording) or the project changed; stop always. No HISTORY row either way."""
    project = fixture.set["name"]
    if args.get("action") == "stop":
        if args.get("preview"):
            return {"state": "preview", "text": f"Stop {project}"}
        fixture.playing = False
        return {"state": "stopped", "position_beats": 0}
    if args.get("from_slot", 1) < 1:
        return error("invalid", "from_slot counts from 1.")
    if fixture.options.recording:
        return error("recording", f"{DAW_NAMES[fixture.daw]} is recording, or record is armed: Kumi never starts "
                                  "playback that would record.")
    if args.get("expected_project_key") != fixture.set["key"]:
        return error("stale", "That isn't the project open now: observe it again.")
    if "from_bar" in args:
        text = f"Play {project} from bar {args['from_bar']}"
    elif "from_beat" in args:
        text = f"Play {project} from beat {args['from_beat']:g}"
    elif "from_slot" in args:
        line = f", line {args['from_line']}" if "from_line" in args else ""
        text = f"Play {project} from sequence slot {args['from_slot']}{line}"
    elif "from_line" in args:
        text = f"Play {project} from line {args['from_line']} of the pattern being edited"
    elif "clip_ref" in args:
        text = f"Launch a Session clip in {project}"
    elif "scene_index" in args:
        text = f"Fire scene {args['scene_index'] + 1} in {project}"
    else:
        text = f"Play {project}"
    if args.get("preview"):
        return {"state": "preview", "text": text}
    fixture.playing = True
    return {"state": "playing", "position_beats": 0}


# CC-24's variations op, as serve's (#1154) routes the DAW's own variations tool.
VARIATIONS_ROOM = 6  # the empty places the fixture's Set has for takes


def drawn_takes(fixture: Fixture, args: dict) -> list[dict]:
    """The takes a make would draw now: numbered on from the set, so a make in between changes them (W1-a). Each line
    in serve's words: the take, its direction, then what was done."""
    recipes = args.get("takes") or [{"direction": f"take {index + 1}"} for index in range(int(args.get("count") or 2))]
    drawn = []
    for index, recipe in enumerate(recipes):
        take, direction = f"V{fixture.takes + index + 1}", recipe.get("direction") or "a take"
        drawn.append({"take": take, "direction": direction, "line": f"{take} ({direction}): 8 bars, 31 notes"})
    return drawn


def takes_digest(takes: list[dict], seed: int) -> str:
    return hashlib.sha256(json.dumps([seed, takes], sort_keys=True).encode("utf-8")).hexdigest()


def variations(fixture: Fixture, args: dict, host: dict) -> dict:
    action = args.get("action")
    if action in ("drop", "extract") or "force" in args or "listen" in args:
        return {"state": "refused", "code": "not_allowed", "reason": "Kumi for Hermes doesn't do that with takes.",
                "history_ids": []}
    if action == "list":
        return {"set": 1, "source": "Bassline", "takes": [{"take": f"V{number}"} for number in range(1, fixture.takes + 1)]}
    project = {"key": fixture.set["key"], "epoch": fixture.epoch}
    if (args.get("expected_project_key"), str(args.get("expected_epoch"))) != (fixture.set["key"], fixture.epoch):
        return {"state": "refused", "code": "stale", "reason": "The project or its epoch changed since it was read.",
                "history_ids": []}
    if action == "keep":
        names = [name for name in args.get("names") or [] if isinstance(name, str)]
        if args.get("preview"):
            return {"state": "preview", "takes": [{"take": name, "name_after": f"{name} · Bassline · kept"}
                                                  for name in names], "skipped": [], "project": project}
        return {"state": "applied", "set": 1, "kept": names, "history_ids": variation_rows(fixture, names, host),
                "undo_available": True, "project": project}
    takes, seed = drawn_takes(fixture, args), int(args.get("seed") or 7)
    if len(takes) > VARIATIONS_ROOM:
        return {"state": "refused", "code": "no_room", "fits": VARIATIONS_ROOM, "history_ids": [],
                "reason": f"Only {VARIATIONS_ROOM} of the {len(takes)} takes fit: the Set has {VARIATIONS_ROOM} empty "
                          f"slots. Ask for {VARIATIONS_ROOM}, or make room in Live first."}
    if args.get("preview"):
        return {"state": "preview", "seed": seed, "source": "“Bassline” on Bass", "takes": takes, "skipped": [],
                "takes_digest": takes_digest(takes, seed), "fits": len(takes), "project": project}
    if args.get("expect_takes") != takes_digest(takes, seed):
        return {"state": "refused", "code": "stale", "reason": "The takes would come out differently now: preview them again.",
                "history_ids": []}
    ids = variation_rows(fixture, [take["take"] for take in takes], host)
    fixture.takes += len(takes)
    return {"state": "applied", "set": 1, "source": "“Bassline” on Bass",
            "takes": [{**take, "ref": f"t2/s{index + 1}"} for index, take in enumerate(takes)], "skipped": [],
            "history_ids": ids, "undo_available": True, "project": project}


def variation_rows(fixture: Fixture, names: list[str], host: dict) -> list[str]:
    """A HISTORY row for each take a call wrote, all of one change (W1-d: "undo that" takes the whole call back)."""
    change_id, ids = f"x-{fixture.generation}-v{len(fixture.history) + 1}", []
    for name in names:
        ident = f"h-{fixture.set['key']}-{fixture.generation}-c{len(fixture.history) + 1}"
        fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}", "project_key": fixture.set["key"],
                                "runtime_generation": fixture.generation, "title": f"Variation {name}",
                                "family": "clip", "at": now(), "state": "applied", "undo_available": True,
                                "track": "Bass", "tool_call_id": host.get("tool_call_id"), "call_outcome": "applied",
                                "change_id": change_id, "before": copy.deepcopy(fixture.set)})
        ids.append(ident)
    fixture.publish("history", "focus")
    return ids


def judged(name: str):
    """Serve's judge and tune as they answer (the plugin-parity contract): refused with the project or a reference
    string, else `done` with Live's round. A change round or a tune is a HISTORY change of its own; a change round
    whose change says "worse" takes the newest change back instead and makes no row."""
    def run(fixture: Fixture, args: dict, host: dict) -> dict:
        same = {"history_ids": [], "taken_back": [], "project_key": fixture.set["key"], "epoch": fixture.epoch}
        if not fixture.connected:
            return {"state": "refused", "code": "not_connected", "reason": "Kumi isn't connected.", **same}
        if (args.get("expected_project_key"), args.get("expected_epoch")) != (fixture.set["key"], fixture.epoch):
            return {"state": "refused", "code": "stale", "reason": "The project changed: observe again.", **same}
        if isinstance((args.get("goal") or {}).get("reference"), str):
            return {"state": "refused", "code": "invalid", "reason": "A reference is a clip_ref or a source.", **same}
        ids, taken = [], []
        if name == "judge" and "worse" in str(args.get("change", "")):
            newest = next(row for row in reversed(fixture.history) if row["state"] == "applied")
            newest["state"], taken = "undone", [newest["id"]]
        elif name == "tune" or "change" in args:
            ident = f"h-{fixture.set['key']}-{fixture.generation}-c{len(fixture.history) + 1}"
            fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}",
                                    "project_key": fixture.set["key"], "runtime_generation": fixture.generation,
                                    "title": "Tune" if name == "tune" else "Judged round", "family": "device",
                                    "at": now(), "state": "applied", "undo_available": True, "track": "Bass",
                                    "tool_call_id": host.get("tool_call_id"), "call_outcome": "applied",
                                    "change_id": f"ch-{len(fixture.history) + 1}", "before": copy.deepcopy(fixture.set)})
            ids = [ident]
        fixture.publish("history", "focus")
        return {**same, "state": "done", "round": {"kind": name, "kept": not taken, "next": "loudness"},
                "history_ids": ids, "taken_back": taken, "undo_available": bool(ids),
                "in_hermes": "In Kumi for Hermes: judge is kumi_judge, tune is kumi_tune."}
    return run


def wavetable(fixture: Fixture, args: dict, host: dict) -> dict:
    """Serve's wavetable: a new file, "Name.wav", else "Name 2.wav"…, never over one; a path is refused."""
    if set(args) != {"device_ref", "wavetable"} or "file" in (args["wavetable"].get("from_audio") or {}):
        return error("invalid", "A wavetable takes device_ref and wavetable only, and from_audio a source.")
    base = args["wavetable"].get("name") or "Kumi Wavetable"
    made = next(label for label in (base, *(f"{base} {n}" for n in range(2, 1000))) if label not in fixture.wavetables)
    fixture.wavetables.add(made)
    return {"made": made, "file": f"{made}.wav", "folder": "Vital's wavetable folder", "frames": 4,
            "next": "Pick it in the plug-in's wavetable menu.",
            "in_hermes": "The file stays where it is: an undo doesn't remove it, and it's not in HISTORY."}


TOOLS = {"status": status, "connect": connect, "observe": observe, "read": read, "change": change, "undo": undo,
         "history": history, "library": library, "sounds": sounds, "listen": listen, "job": job,
         "setup_daw": setup_daw, "generate": generate, "transport": transport, "script": script,
         "variations": variations, "device": device, "preset_build": preset_build,
         "preset": preset, "judge": judged("judge"), "tune": judged("tune"), "wavetable": wavetable}


def main() -> None:
    # Serve's stdio is UTF-8 everywhere; a pipe's default here is the locale's (cp1252 on Windows), which would read a
    # script's U+202E as other characters and hash different code than the plugin sent.
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
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
    parser.add_argument("--recording", action="store_true", help="the DAW records, so transport refuses play")
    parser.add_argument("--stderr-flood", type=int, default=0, help="bytes written to stderr on start")
    parser.add_argument("--close-stdout", action="store_true", help="close stdout after initialize, and stay")
    parser.add_argument("--ignore-eof", action="store_true", help="stay after stdin ends")
    parser.add_argument("--generation", default="g1", help="this runtime's generation, as HISTORY's ids carry it")
    parser.add_argument("--no-resources", action="store_true", help="offer no resources, as kumi serve before #1047")
    parser.add_argument("--listen-meta", help="JSON listen tool metadata advertised by this runtime")
    parser.add_argument("--no-tools-list", action="store_true", help="an older runtime without tools/list")
    parser.add_argument("--refuse-subscribe", action="append", default=[], help="refuse a subscription to this URI")
    parser.add_argument("--quiet", action="append", default=[], help="never tell a subscriber this URI changed")
    parser.add_argument("--publish-at-start", action="store_true", help="write HISTORY and FOCUS as it starts")
    parser.add_argument("--library-root", action="append", default=[])
    parser.add_argument("--hermes-home", help="the profile's Hermes home, as kumi serve takes it (#985)")
    parser.add_argument("--hermes-root", help="the root holding every Hermes profile, as kumi serve takes it (#985)")
    parser.add_argument("--media-root", action="append", default=[])
    parser.add_argument("--listen-outcome", default="measured",
                        choices=["measured", "refused", "cancelled", "unsure", "unsure-with-media"])
    parser.add_argument("--listen-deadline", help="the preview's deadline_seconds, as JSON, or missing (default: 75, "
                                                  "the capture's 65 s cap, restoring and measuring)")
    parser.add_argument("--listen-prepare", help="the preview's prepare_seconds, as JSON, or missing (default: 0, or "
                                                 "120 with compare_to)")
    parser.add_argument("--listen-estimated", help="the preview's estimated_seconds, as JSON, or missing (default: "
                                                   "the window's)")
    parser.add_argument("--component-state", default="missing",
                        choices=["missing", "same", "older", "newer", "other_owner"])
    parser.add_argument("--component-owner")
    parser.add_argument("--generate-estimate", type=int, default=15, help="a quote's estimate, in credits")
    parser.add_argument("--generate-create", default="running",
                        choices=["running", "not_submitted", "ledger_busy_once", "ledger_busy"])
    parser.add_argument("--generate-actual", type=int, default=12, help="a finished job's charge (-1: unknown)")
    parser.add_argument("--generate-expires-ms", type=int, default=300_000,
                        help="how long a quote lasts from now, in milliseconds (negative: already expired)")
    parser.add_argument("--device-wrong-sha", action="store_true",
                        help="a device preview answers another SHA-256, as a serve that re-encoded the spec would")
    parser.add_argument("--device-file-appears", action="store_true",
                        help="a file takes the previewed device's name before its build")
    parser.add_argument("--preset-unchecked", action="store_true",
                        help="a preset preview couldn't read the installed plug-in's version (version_checked false)")
    parser.add_argument("--preset-changed", action="store_true",
                        help="a preset place finds its preview's digest or file no longer holds (26-r1)")
    parser.add_argument("--preset-settings", type=int, default=0, help="the preset has this many volume changes")
    parser.add_argument("--preset-hidden", action="store_true",
                        help="the preset's name carries a direction-changing character")
    parser.add_argument("--preset-refusal", choices=sorted(PRESET_REFUSALS),
                        help="a preset preview refuses with this code, as serve's preset op can")
    parser.add_argument("--preset-unedited", action="store_true", help="the preset changes nothing in its template")
    parser.add_argument("--preset-shapes", action="store_true",
                        help="a renderer-shape record (each kind of value at once), not a real writer receipt")
    Fixture(parser.parse_args()).serve()


if __name__ == "__main__":
    main()
