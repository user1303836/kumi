"""A stand-in for `kumi serve --stdio` (RESEARCH.md C2, C3), for the plugin's tests and for building UIs without Kumi.

It takes the runtime's arguments, answers MCP over stdin and stdout as the runtime does, and holds a made-up Live Set
("Night Drive") in memory: connect, observe, read, a typed change with HISTORY, undo, and the C3 snapshot files in
`--state-dir`, with a resource update after each. Standard library only.

    python fixture_runtime.py serve --stdio --state-dir DIR --kumi-home DIR --daw ableton
        [--server-version V] [--protocol P] [--exit-on TOOL] [--slow TOOL=SECONDS] [--leased-by SESSION]
        [--refuse-subscribe URI] [--stderr-flood BYTES] [--close-stdout] [--ignore-eof]

The knobs make it a runtime that's the wrong version, crashes or answers late on a tool, holds a lease for another chat,
refuses a subscription, floods its stderr, closes its output and stays, or stays after its stdin ends.

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

VERSION = "1.24.66"
PROTOCOL = "kumi-serve/1"
MCP = "2025-11-25"
RESOURCES = {"status": "kumi://status", "focus": "kumi://focus", "history": "kumi://history"}
# The plan steps the fixture applies, as the real runtime names Kumi's tools (W1-REGISTRY: `{tool, input, as}`).
STEPS = {"set_track_volume", "replace_sample"}
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
        self.seq = 0
        self.connected = False
        self.daw = options.daw
        self.epoch = "1"
        self.set = copy.deepcopy(SET)
        self.history: list[dict] = []
        self.subscribed: set[str] = set()
        self.owner: dict | None = None
        self.slow = dict(item.split("=", 1) for item in options.slow)
        self.writing = threading.Lock()
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
        if self.options.ignore_eof:
            time.sleep(3600)  # stays after its stdin ends, as a runtime that never settles might

    def answer(self, method: str, params: dict) -> dict:
        if method == "initialize":
            return {"protocolVersion": MCP, "serverInfo": {"name": "kumi-serve", "version": self.options.server_version},
                    "capabilities": {"tools": {}, "resources": {"subscribe": True}}}
        if method == "tools/list":
            return {"tools": [{"name": name, "inputSchema": {"type": "object"}} for name in TOOLS]}
        if method == "resources/list":
            return {"resources": [{"uri": uri, "name": name} for name, uri in RESOURCES.items()]}
        if method == "resources/subscribe":
            if params.get("uri") in self.options.refuse_subscribe:
                raise LookupError(f"no subscription to {params.get('uri')}")
            self.subscribed.add(params["uri"])
            return {}
        if method == "tools/call":
            return self.call(params["name"], dict(params.get("arguments") or {}))
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
        temporary = path.with_suffix(path.suffix + ".tmp")
        temporary.write_text(json.dumps(value, indent=1), encoding="utf-8")
        os.replace(temporary, path)

    def publish(self, *names: str) -> None:
        for name in names:
            self.seq += 1
            body = {"status": self.status, "focus": self.focus, "history": self.history_snapshot}[name]()
            self.record(f"{name}.json", {"schema": 1, "seq": self.seq, "at": now(), "owner": self.owner, **body})
            if RESOURCES[name] in self.subscribed:
                self.send({"jsonrpc": "2.0", "method": "notifications/resources/updated",
                           "params": {"uri": RESOURCES[name]}})

    def project(self) -> dict | None:
        return {"name": self.set["name"], "key": self.set["key"], "epoch": self.epoch} if self.connected else None

    def status(self) -> dict:
        return {"runtime": {"version": self.options.server_version, "protocol": self.options.protocol},
                "connected": self.connected, "stale": False, "daw": self.daw, "project": self.project(),
                "owner": self.owner, "capabilities": ["observe", "read", "change", "undo", "history"]}

    def focus(self) -> dict:
        drums = self.set["tracks"][0]
        return {"project": self.project(), "focus": {"track": drums["name"], "device": "Drum Rack", "clip": None,
                                                     "view": "arrangement"},
                "transport": {"playing": False, "tempo": self.set["tempo"], "position": 0.0}, "catch_up": []}

    def history_snapshot(self) -> dict:
        """HISTORY, newest first, as C3 has it: each row without the Set the fixture keeps to undo it."""
        return {"items": [{key: value for key, value in row.items() if key != "before"}
                          for row in reversed(self.history)][:200]}

    def track(self, ref: str) -> dict | None:
        return next((track for track in self.set["tracks"] if track["ref"] == ref), None)


def now() -> int:
    return int(time.time() * 1000)


def error(code: str, reason: str) -> dict:
    return {"error": {"code": code, "reason": reason}}


def status(fixture: Fixture, args: dict, host: dict) -> dict:
    return fixture.status()


def connect(fixture: Fixture, args: dict, host: dict) -> dict:
    if args.get("daw", fixture.daw) != fixture.daw:
        return error("other_daw", f"kumi serve was started for {fixture.daw}")
    fixture.connected = True
    fixture.owner = {"profile": host.get("profile_id"), "session_id": host.get("session_id"), "pid": os.getpid()}
    fixture.seq += 1
    fixture.record("owner.json", {"schema": 1, "seq": fixture.seq, "at": now(), "owner": fixture.owner,
                                  "daw": fixture.daw, "heartbeat": now()})
    fixture.publish("status", "focus", "history")
    return fixture.status()


def observe(fixture: Fixture, args: dict, host: dict) -> dict:
    if not fixture.connected:
        return error("not_connected", "Kumi isn't connected to the DAW: connect first.")
    tracks = "; ".join(f"{track['name']} ({track['ref']}, {track['volume_db']:g} dB)" for track in fixture.set["tracks"])
    text = f"{fixture.set['name']}, {fixture.set['tempo']:g} BPM. Tracks: {tracks}. FOCUS: Drums > Drum Rack."
    return {"text": text[:int(args.get("max_chars") or 4000)], "project_key": fixture.set["key"],
            "focus": fixture.focus()["focus"], "catch_up": [], "epoch": fixture.epoch, "age_ms": 0,
            "refs": {track["ref"]: track["name"] for track in fixture.set["tracks"]}}


def read(fixture: Fixture, args: dict, host: dict) -> dict:
    if not fixture.connected:
        return error("not_connected", "Kumi isn't connected to the DAW: connect first.")
    if args.get("action") == "schema":
        if args.get("tool") not in STEPS:
            return error("unsupported", f"{args.get('tool')} isn't a step Kumi takes here")
        return {"tool": args["tool"], "schema": {"type": "object"}}
    return {"rows": copy.deepcopy(fixture.set["tracks"]), "cursor": None, "epoch": fixture.epoch}


def change(fixture: Fixture, args: dict, host: dict) -> dict:
    if fixture.options.leased_by:
        owner = {"profile": "default", "session_id": fixture.options.leased_by, "pid": 4242, "since": now()}
        return {"error": {"code": "leased", "reason": "Another chat holds the DAW for changes.", "owner": owner}}
    refused = {"state": "refused", "history_ids": [], "steps": [], "summary": "Nothing changed.", "undo_available": False}
    if (args.get("expected_project_key"), str(args.get("expected_epoch"))) != (fixture.set["key"], fixture.epoch):
        return {**refused, "reason": "stale: the project or its epoch changed since it was read; read it again"}
    steps = args.get("steps") or []
    unknown = [step.get("tool") for step in steps if step.get("tool") not in STEPS]
    if not steps or len(steps) > 20 or unknown:
        return {**refused, "reason": f"Kumi doesn't take these steps here: {unknown or 'none given'}"}
    before = copy.deepcopy(fixture.set)
    done = []
    for step in steps:
        target = step.get("input") or {}
        track = fixture.track(target.get("track_ref", ""))
        if track is None:
            fixture.set = before
            return {**refused, "reason": f"no track {target.get('track_ref')!r} in the project"}
        if step["tool"] == "set_track_volume":
            track["volume_db"] = float(target["volume_db"])
        else:
            track["devices"][0]["pads"][0]["sample"] = target["sample"]
        done.append({"tool": step["tool"], "state": "applied"})
    ident = f"h-{fixture.set['key']}-g1-c{len(fixture.history) + 1}"
    fixture.history.append({"id": ident, "label": f"c{len(fixture.history) + 1}", "project_key": fixture.set["key"],
                            "runtime_generation": "g1", "title": args.get("title") or "Kumi change",
                            "family": steps[0]["tool"], "at": now(), "state": "applied", "undo_available": True,
                            "track": steps[0]["input"].get("track_ref"), "before": before})
    fixture.publish("history", "focus")
    return {"state": "applied", "history_ids": [ident], "steps": done, "summary": args.get("title") or "Applied.",
            "undo_available": True}


def undo(fixture: Fixture, args: dict, host: dict) -> dict:
    row = next((row for row in fixture.history if row["id"] == args.get("history_id")), None)
    if row is None:
        return {"state": "refused", "reason": "No such change in this runtime's HISTORY."}
    if not row["undo_available"]:
        return {"state": "expired", "reason": "That change can't be undone any more."}
    fixture.set = row.pop("before")
    row.update(state="undone", undo_available=False)
    fixture.publish("history", "focus")
    return {"state": "undone", "reason": f"Undid {row['title']}."}


def history(fixture: Fixture, args: dict, host: dict) -> dict:
    return {"items": fixture.history_snapshot()["items"][:int(args.get("limit") or 20)]}


TOOLS = {"status": status, "connect": connect, "observe": observe, "read": read, "change": change, "undo": undo,
         "history": history}


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
    parser.add_argument("--refuse-subscribe", action="append", default=[])
    parser.add_argument("--stderr-flood", type=int, default=0, help="bytes written to stderr on start")
    parser.add_argument("--close-stdout", action="store_true", help="close stdout after initialize, and stay")
    parser.add_argument("--ignore-eof", action="store_true", help="stay after stdin ends")
    Fixture(parser.parse_args()).serve()


if __name__ == "__main__":
    main()
