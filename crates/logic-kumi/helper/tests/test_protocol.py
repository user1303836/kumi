"""The helper's protocol, driven over its stdin and stdout: ids, errors, the ops that need no Accessibility grant,
the virtual MIDI ports both ways, the process tap on afplay, and a clean exit. Everything that needs the grant is
checked for a well-formed error only, so the suite passes on a Mac without Logic and without the grant.

The tap's tests play a quiet tone (-30 dBFS, after a second of silence) with afplay and tap it muted; they never tap
a DAW. KUMI_TAP_DIR names the folder their files go in (a temporary one inside it, removed after); without it, the
system's temporary folder."""
import array
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import queue
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
SPEC = importlib.util.spec_from_file_location("build_logic_hands", ROOT / "scripts/build-logic-hands.py")
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)

# Nothing the suite sends should matter to a DAW listening on every MIDI input: Active Sensing is ignored everywhere.
PORT = "Kumi helper test"
INERT = "FE"
HELPER = None
ECHO = None

def setUpModule():
    global HELPER, ECHO
    if sys.platform != "darwin":
        raise unittest.SkipTest("the helper is macOS only")
    HELPER = builder.build(reuse=True)
    ECHO = build_echo()

def build_echo() -> Path:
    """The tests' MIDI counterpart, built once per source."""
    source = HERE / "midi_echo.swift"
    target = ROOT / "target/logic-hands-tests" / ("midi-echo-" + hashlib.sha256(source.read_bytes()).hexdigest()[:12])
    if not target.is_file():
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["xcrun", "swiftc", "-swift-version", "5", "-o", str(target), str(source)], check=True)
    return target

def missing_pid() -> int:
    pid = 99_999
    while True:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return pid
        except PermissionError:
            pass
        pid -= 1

class Helper:
    """One helper process: requests with their replies, and the events that came in between."""
    def __init__(self):
        self.process = subprocess.Popen([str(HELPER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        self.lines = queue.Queue()
        self.events = []
        self.next_id = 0
        self.stderr = ""
        threading.Thread(target=self._pump, daemon=True).start()
        threading.Thread(target=self._drain, daemon=True).start()

    def _pump(self):
        for line in self.process.stdout:
            self.lines.put(line)

    def _drain(self):
        self.stderr = self.process.stderr.read()

    def write(self, line: str) -> None:
        self.process.stdin.write(line + "\n")
        self.process.stdin.flush()

    def read(self, timeout=3.0) -> dict:
        try:
            line = self.lines.get(timeout=timeout)
        except queue.Empty:
            raise AssertionError(f"no line from the helper within {timeout} s; stderr: {self.stderr!r}")
        message = json.loads(line)
        assert line.endswith("\n") and "\n" not in line[:-1], "one JSON object a line"
        return message

    def reply(self, timeout=3.0) -> dict:
        """The next reply, keeping events aside."""
        while True:
            message = self.read(timeout)
            if "event" in message:
                self.events.append(message)
                continue
            return message

    def request(self, op: str, request_id=None, **params) -> dict:
        self.next_id += 1
        request_id = self.next_id if request_id is None else request_id
        started = time.monotonic()
        self.write(json.dumps({"id": request_id, "op": op, **params}))
        message = self.reply()
        assert time.monotonic() - started < 2.5, f"{op} took longer than the 2 s the contract allows"
        assert message.get("id") == request_id, f"{op} answered id {message.get('id')!r}, not {request_id!r}"
        assert isinstance(message.get("ok"), bool) and isinstance(message.get("ms"), int), message
        return message

    def events_for(self, seconds: float) -> list:
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            try:
                message = json.loads(self.lines.get(timeout=0.05))
            except queue.Empty:
                continue
            if "event" in message:
                self.events.append(message)
        return self.events

    def close(self) -> int:
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            return self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            raise AssertionError("the helper did not exit after EOF")
        finally:
            self.process.stdout.close()
            self.process.stderr.close()

def write_tone(path: Path, lead: float, seconds: float, frequency=440.0, level_db=-30.0, rate=48_000) -> None:
    """A 32-bit float stereo WAV: `lead` seconds of silence, then a sine at `level_db` dBFS with 10 ms fades."""
    amplitude = 10 ** (level_db / 20)
    fade = int(0.01 * rate)
    length = int(seconds * rate)
    samples = array.array("f", bytes(8 * int(lead * rate)))
    for index in range(length):
        value = amplitude * min(1.0, index / fade, (length - 1 - index) / fade) * math.sin(2 * math.pi * frequency * index / rate)
        samples.extend((value, value))
    data = samples.tobytes()
    header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 3, 2, rate, rate * 8, 8, 32)
    path.write_bytes(header + b"data" + struct.pack("<I", len(data)) + data)

def read_capture(path: Path):
    """A WAV the tap wrote, its header checked against the file: (rate, channels, samples)."""
    data = path.read_bytes()
    assert data[:4] == b"RIFF" and data[8:16] == b"WAVEfmt " and data[36:40] == b"data", data[:44]
    code, channels, rate, byte_rate, block, bits = struct.unpack("<HHIIHH", data[20:36])
    size = struct.unpack("<I", data[40:44])[0]
    assert (code, bits, block, byte_rate) == (3, 32, 4 * channels, 4 * channels * rate), (code, bits, block, byte_rate)
    assert struct.unpack("<I", data[4:8])[0] == 36 + size and len(data) == 44 + size, (data[4:8], size, len(data))
    samples = array.array("f")
    samples.frombytes(data[44:])
    return rate, channels, samples

def ok(message: dict) -> dict:
    assert message["ok"] is True, message
    return message

def error(message: dict) -> str:
    assert message["ok"] is False, message
    assert isinstance(message.get("error"), str) and message["error"], message
    return message["error"]

class Protocol(unittest.TestCase):
    def setUp(self):
        self.helper = Helper()

    def tearDown(self):
        self.helper.close()

    def test_ids_are_echoed_as_given(self):
        for request_id in (1, 42, "seven", 2 ** 40):
            self.assertEqual(ok(self.helper.request("trusted", request_id=request_id, prompt=False))["id"], request_id)
        self.helper.write(json.dumps({"op": "trusted", "prompt": False}))
        self.assertIsNone(self.helper.reply()["id"])

    def test_unknown_op_is_an_error_with_the_id(self):
        message = self.helper.request("nope", request_id=9)
        self.assertEqual(error(message), "unknown-op")
        self.assertEqual(message["id"], 9)

    def test_malformed_line_is_an_error_reply_and_service_goes_on(self):
        self.helper.write("not json")
        message = self.helper.reply()
        self.assertEqual(message, {"id": None, "ok": False, "error": "malformed"})
        self.helper.write("[1, 2]")
        self.assertEqual(error(self.helper.reply()), "malformed")
        self.helper.write("")
        ok(self.helper.request("trusted", prompt=False))

    def test_trusted_answers_a_boolean_without_prompting(self):
        self.assertIsInstance(ok(self.helper.request("trusted", prompt=False))["trusted"], bool)
        self.assertIsInstance(ok(self.helper.request("trusted"))["trusted"], bool)

    def test_apps_of_an_absent_bundle_is_empty(self):
        self.assertEqual(ok(self.helper.request("apps", bundles=["com.example.absent.kumi"]))["apps"], [])
        self.assertEqual(ok(self.helper.request("apps", bundles=[]))["apps"], [])
        self.assertEqual(error(self.helper.request("apps")), "bad-request")

    def test_apps_describes_a_running_app(self):
        apps = ok(self.helper.request("apps", bundles=["com.apple.finder"]))["apps"]
        for app in apps:
            self.assertEqual(set(app), {"bundle", "pid", "name", "active", "hidden"})
            self.assertEqual(app["bundle"], "com.apple.finder")
            self.assertIsInstance(app["pid"], int)
            self.assertIsInstance(app["active"], bool)
            self.assertIsInstance(app["hidden"], bool)

    def test_processes_finds_this_interpreter(self):
        pids = ok(self.helper.request("processes", contains="ython"))["pids"]
        self.assertIn(os.getpid(), pids)
        self.assertEqual(pids, sorted(pids))
        self.assertEqual(ok(self.helper.request("processes", contains="/no/such/path/kumi"))["pids"], [])
        self.assertEqual(error(self.helper.request("processes")), "bad-request")

    def test_windows_of_a_pid_without_windows_is_empty(self):
        message = self.helper.request("windows", pid=os.getpid())
        if message["ok"]:
            self.assertEqual(message["windows"], [])
            self.assertEqual(message["listed"], [])
        else:
            self.assertEqual(error(message), "untrusted")
        self.assertEqual(error(self.helper.request("windows", pid=missing_pid())), "no-process" if ok(self.helper.request("trusted"))["trusted"] else "untrusted")
        self.assertEqual(error(self.helper.request("windows")), "bad-request")

    def test_tree_and_get_of_a_pid_without_windows(self):
        message = self.helper.request("tree", pid=os.getpid(), depth=3)
        if message["ok"]:
            self.assertEqual(message["node"], {})
        else:
            self.assertEqual(error(message), "untrusted")
        self.assertIn(error(self.helper.request("tree", pid=os.getpid(), attrs=["bogus"])), ("unknown-attr", "untrusted"))
        self.assertIn(error(self.helper.request("get", pid=os.getpid(), root="main", attrs=["AXTitle"])), ("no-root", "untrusted"))
        self.assertIn(error(self.helper.request("get", pid=os.getpid(), root="nowhere", attrs=["AXTitle"])), ("bad-request", "untrusted"))
        self.assertIn(error(self.helper.request("set", pid=os.getpid(), attr="AXValue")), ("bad-request", "untrusted"))
        self.assertIn(error(self.helper.request("action", pid=os.getpid(), path="0")), ("bad-request", "no-element", "untrusted"))
        self.assertIn(error(self.helper.request("menu", pid=os.getpid(), titles=["Window"])), ("no-menubar", "untrusted"))
        self.assertIn(error(self.helper.request("menu", pid=os.getpid(), titles=[])), ("bad-request", "untrusted"))

    def test_keys_to_a_missing_process_is_an_error(self):
        pid = missing_pid()
        self.assertEqual(error(self.helper.request("keys", pid=pid, key="a", mods=["cmd"])), "no-process")
        self.assertEqual(error(self.helper.request("type", pid=pid, text="hello")), "no-process")
        self.assertEqual(error(self.helper.request("activate", pid=pid)), "no-app")
        self.assertEqual(error(self.helper.request("keys", pid=os.getpid(), key="bogus")), "unknown-key")
        self.assertEqual(error(self.helper.request("keys", pid=os.getpid(), key="a", mods=["hyper"])), "unknown-mod")
        self.assertEqual(error(self.helper.request("keys", key="a")), "bad-request")
        # A click needs a point and a process; its grant is checked after those.
        self.assertEqual(error(self.helper.request("click", pid=os.getpid(), x=10)), "bad-request")
        self.assertEqual(error(self.helper.request("click", pid=pid, x=10, y=10)), "no-process")
        self.assertEqual(error(self.helper.request("click", pid=os.getpid(), x=10, y=10, mods=["hyper"])), "unknown-mod")
        self.assertEqual(error(self.helper.request("type", pid=os.getpid(), text="x" * 201)), "too-long")

    def test_midi_ports_round_trip(self):
        self.assertEqual(error(self.helper.request("midi.send", hex=INERT)), "midi-closed")
        opened = ok(self.helper.request("midi.open", name=PORT, uid=1_234_567))
        self.assertEqual((opened["opened"], opened["source"], opened["destination"]), (True, 1_234_567, 1_234_568))
        self.assertEqual(error(self.helper.request("midi.open", name=PORT, uid=1_234_567)), "midi-open")
        # The echo sends the inert byte to the helper's destination, then listens on its source.
        with subprocess.Popen([str(ECHO), PORT, INERT, "2"], stdout=subprocess.PIPE, text=True) as echo:
            self.assertEqual(echo.stdout.readline().strip(), "ready")
            self.assertTrue(ok(self.helper.request("midi.send", hex=INERT))["sent"])
            self.assertEqual(error(self.helper.request("midi.send", hex="F")), "bad-hex")
            # A DAW that is open scans every new port for a control surface (Logic sends its Mackie device queries,
            # F0 00 00 66 …); those aren't the echo's and are left out.
            events = [event for event in self.helper.events_for(1.0) if event["event"] == "midi" and not event["hex"].startswith("F0 00 00 66")]
            self.assertEqual([event["hex"] for event in events], [INERT])
            self.assertIsInstance(events[0]["ms"], int)
            self.assertGreater(events[0]["ms"], 1_700_000_000_000)
            # The host times the tap's anchors are in: when the helper got the packet, and the packet's own stamp.
            self.assertGreater(events[0]["hostTime"], 0)
            self.assertIsInstance(events[0]["timeStamp"], int)
            heard = echo.stdout.read().split("\n")
        self.assertIn(INERT, heard)
        self.assertTrue(ok(self.helper.request("midi.close"))["closed"])
        self.assertTrue(ok(self.helper.request("midi.close"))["closed"])
        gone = subprocess.run([str(ECHO), PORT, "", "0.1"], stdout=subprocess.PIPE, text=True)
        self.assertEqual(gone.returncode, 3, gone.stdout)
        # Named by text, the ids come from a hash of it and stay the same from one run to the next.
        opened = ok(self.helper.request("midi.open", name=PORT, uid=PORT))
        self.assertEqual((opened["source"], opened["destination"]), (1_745_627_942, 1_745_627_943))
        self.assertTrue(ok(self.helper.request("midi.close"))["closed"])

    def test_quit_answers_then_exits(self):
        self.helper.write(json.dumps({"id": 5, "op": "quit"}))
        self.assertEqual(self.helper.reply(), {"id": 5, "ok": True})
        self.assertEqual(self.helper.process.wait(timeout=5), 0)

    def test_eof_exits_cleanly(self):
        ok(self.helper.request("trusted", prompt=False))
        self.assertEqual(self.helper.close(), 0)
        self.assertEqual(self.helper.stderr, "")

class Tap(unittest.TestCase):
    """The process tap, on afplay playing a quiet tone the tap mutes: never on a DAW."""
    def setUp(self):
        self.helper = Helper()
        self.folder = tempfile.TemporaryDirectory(prefix="kumi-tap-", dir=os.environ.get("KUMI_TAP_DIR"))
        self.files = Path(self.folder.name)
        self.players = []

    def tearDown(self):
        for player in self.players:
            if player.poll() is None:
                player.kill()
            player.wait()
        if self.helper.process.poll() is None:
            self.helper.close()
        self.folder.cleanup()

    def play(self, lead: float, seconds: float) -> subprocess.Popen:
        if shutil.which("afplay") is None:
            self.skipTest("afplay isn't on this Mac")
        tone = self.files / f"tone-{len(self.players)}.wav"
        write_tone(tone, lead, seconds)
        player = subprocess.Popen(["afplay", str(tone)])
        self.players.append(player)
        return player

    def start(self, player: subprocess.Popen, capture: Path) -> dict:
        return self.start_with(player, capture, maxSeconds=30)

    def start_with(self, player: subprocess.Popen, capture: Path, maxSeconds: float) -> dict:
        """tap.start on the player, muted, once Core Audio has a process object for it (a few tens of milliseconds in);
        a tap macOS denies, or one too old a macOS can't make, skips the test."""
        until = time.monotonic() + 2.0
        while True:
            message = self.helper.request("tap.start", pid=player.pid, path=str(capture), mute=True, maxSeconds=maxSeconds)
            if message["ok"]:
                return message
            if message["error"] in ("denied", "unsupported"):
                player.kill()
                self.skipTest(f"the tap is {message['error']}: {message.get('detail')}")
            if message["error"] != "failed" or message.get("step") != "process" or time.monotonic() > until:
                self.fail(f"tap.start: {message}")
            time.sleep(0.01)

    def test_requests_are_checked_and_nothing_is_tapped_without_a_process_object(self):
        self.assertEqual(error(self.helper.request("tap.stop")), "failed")
        self.assertEqual(ok(self.helper.request("tap.status"))["running"], False)
        capture = self.files / "capture.wav"
        for params in ({"path": str(capture)}, {"pid": os.getpid()}, {"pid": os.getpid(), "path": "capture.wav"},
                       {"pid": os.getpid(), "path": str(capture), "mute": "yes"}, {"pid": os.getpid(), "path": str(capture), "maxSeconds": 0},
                       {"pid": os.getpid(), "path": str(capture), "maxSeconds": 3601}):
            self.assertEqual(error(self.helper.request("tap.start", **params)), "bad-request", params)
        # This interpreter makes no audio, so Core Audio has no process object for it; a pid nobody has is no process.
        for pid in (os.getpid(), missing_pid()):
            message = self.helper.request("tap.start", pid=pid, path=str(capture))
            if error(message) != "unsupported":
                self.assertEqual((message["error"], message["step"]), ("failed", "process"), message)
        self.assertFalse(capture.exists())
        self.assertEqual(ok(self.helper.request("tap.status"))["running"], False)

    def test_a_capture_of_afplay_is_a_float_wav_with_anchors(self):
        player = self.play(lead=1.0, seconds=1.0)
        capture = self.files / "capture.wav"
        started = self.start(player, capture)
        self.assertEqual(started["channels"], 2)
        self.assertGreater(started["sampleRate"], 0)
        self.assertIsInstance(started["hostTime"], int)
        self.assertLess(started["ms"], 2000)
        # One tap at a time, and it says how far it is.
        self.assertEqual(error(self.helper.request("tap.start", pid=player.pid, path=str(self.files / "other.wav"))), "busy")
        self.assertFalse((self.files / "other.wav").exists())
        self.assertEqual(ok(self.helper.request("tap.status"))["running"], True)
        player.wait(timeout=10)
        stopped = ok(self.helper.request("tap.stop"))
        self.assertEqual(error(self.helper.request("tap.stop")), "failed")
        self.assertEqual(set(stopped) - {"id", "ok", "ms"}, {"file", "frames", "seconds", "sampleRate", "channels", "peak", "rms", "anchors", "timebase", "dropped", "truncated"})
        self.assertEqual((stopped["file"], stopped["channels"], stopped["sampleRate"]), (str(capture), 2, started["sampleRate"]))
        self.assertEqual((stopped["dropped"], stopped["truncated"]), (0, False))
        self.assertGreater(stopped["seconds"], 1.5)
        rate, channels, samples = read_capture(capture)
        self.assertEqual((rate, channels, len(samples) // channels), (round(stopped["sampleRate"]), 2, stopped["frames"]))
        peak = max(abs(sample) for sample in samples)
        self.assertAlmostEqual(peak, stopped["peak"], places=6)
        self.assertLess(abs(20 * math.log10(peak) + 30), 1.0, f"the tone's peak is {20 * math.log10(peak):.2f} dBFS, not -30")
        # The anchors: frames and host times both rising, the first at frame 0, agreeing with the rate within 1 ms.
        anchors, timebase = stopped["anchors"], stopped["timebase"]
        self.assertGreaterEqual(len(anchors), 2)
        self.assertEqual(anchors[0][0], 0)
        tick = timebase["numer"] / timebase["denom"] / 1e9
        for before, after in zip(anchors, anchors[1:]):
            self.assertGreater(after[0], before[0])
            self.assertGreater(after[1], before[1])
            self.assertLess(abs((after[1] - anchors[0][1]) * tick - after[0] / stopped["sampleRate"]), 0.001)

    def test_a_capture_stops_writing_at_max_seconds(self):
        # Reaching maxSeconds stops the tap's IO, which ends the mute: the player goes long before its tone would begin.
        player = self.play(lead=2.0, seconds=0.5)
        capture = self.files / "capture.wav"
        started = self.start_with(player, capture, maxSeconds=0.3)
        time.sleep(0.7)
        status = ok(self.helper.request("tap.status"))
        self.assertEqual((status["running"], status["truncated"]), (True, True))
        stopped = ok(self.helper.request("tap.stop"))
        player.kill()
        self.assertEqual(stopped["truncated"], True)
        self.assertEqual(stopped["frames"], int(0.3 * started["sampleRate"]))
        rate, channels, samples = read_capture(capture)
        self.assertEqual(len(samples) // channels, stopped["frames"])

    def test_eof_or_a_signal_finishes_a_running_tap(self):
        # Two seconds of silence: the tap ends and the player is stopped before the tone would begin.
        for ending in ("eof", "sigterm"):
            if ending == "sigterm":
                self.helper = Helper()
            player = self.play(lead=2.0, seconds=0.5)
            capture = self.files / f"capture-{ending}.wav"
            self.start(player, capture)
            time.sleep(0.4)
            if ending == "eof":
                self.assertEqual(self.helper.close(), 0)
            else:
                self.helper.process.send_signal(signal.SIGTERM)
                self.assertEqual(self.helper.process.wait(timeout=5), -signal.SIGTERM)
                self.helper.close()
            player.kill()
            rate, channels, samples = read_capture(capture)
            self.assertEqual(channels, 2)
            self.assertGreater(len(samples) // channels, rate // 10, ending)

class Build(unittest.TestCase):
    def test_helper_is_named_after_its_source(self):
        self.assertEqual(HELPER.name, "kumi-logic-hands-" + hashlib.sha256(builder.SOURCE.read_bytes()).hexdigest()[:12])
        self.assertEqual(sorted(HELPER.parent.iterdir()), [HELPER])

if __name__ == "__main__":
    unittest.main()
