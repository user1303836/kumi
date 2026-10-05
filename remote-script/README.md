# AbletonMcpBridge Remote Script

AbletonMcpBridge is the half of the Ableton bridge that runs inside Live: a
Control Surface script that reads and changes the Set through Live's own
Python API. The other half is the MCP server in the `@ableton-mcp/mcp-server`
package (crates/ableton-mcp-server in the repository), which Kumi and other MCP
clients start. The two talk over an authenticated loopback socket. On Live 12.4
and later, the server also reaches Kumi's Live extension, a separate channel
this script isn't part of.

Repository: https://github.com/user1303836/kumi

## Files

`kumi bridge`, or `ableton-mcp-lifecycle install`, puts the package into the
Remote Scripts folder of Live's User Library, as `Remote Scripts/AbletonMcpBridge`:

| File | What it is |
| --- | --- |
| `__init__.py` | The entry point Live loads: reads the configuration, starts the bridge, owns the optional Willington providers |
| `ableton_mcp_remote_script.py` | The bridge: socket, protocol, and the mapper that reads and changes Live |
| `ableton-live-v1.operations.json` | The operation registry it shares with the server; requests and results are checked against it |
| `manifest.json` | SHA-256 of the three files above and the registry's hash, for the installer's checks |
| `bridge-reference.json` | Points to the bridge configuration |
| `__pycache__` | An empty, read-only file, so Python never writes bytecode beside the checked sources |
| `willington.json` | Optional, yours: see Willington below. Updates keep it. |

This README ships in the bridge's package only; it isn't copied into Live.

After installing, open Live and choose AbletonMcpBridge as a Control Surface in
Settings → Link, Tempo & MIDI, once. Live loads the script when it starts, so
an update needs Live to be closed and opened again.

## Configuration

Live calls `create_instance(c_instance)` with nothing else, so the script reads
its configuration from files, never from environment variables or arguments:

1. `bridge-reference.json`, beside `__init__.py`: `{"config": "<absolute path>"}`
   and nothing else.
2. The bridge configuration it names (`bridge-config.json` in the lifecycle's
   state folder; for Kumi, `~/.kumi/bridge/state`):

   ```json
   {
     "version": 2,
     "server": {"command": "/absolute/path/to/ableton-mcp-server", "args": ["--config", "/absolute/path/to/bridge-config.json"]},
     "bridge": {
       "host": "127.0.0.1",
       "port": 9765,
       "realtimePort": 9766,
       "secretFile": "/absolute/path/to/bridge.secret",
       "timeoutMs": 5000,
       "diagnostics": {"path": "/absolute/path/to/bridge-diagnostics.log", "maxBytes": 16777216}
     }
   }
   ```

   | Field | Meaning |
   | --- | --- |
   | `host` | `127.0.0.1` or `::1`; nothing else is accepted |
   | `port` | The TCP port the script listens on (lifecycle default 9765) |
   | `realtimePort` | Optional UDP port for realtime control, different from `port` (lifecycle default 9766) |
   | `secretFile` | Absolute path to the shared secret |
   | `timeoutMs` | The server's request timeout, 100 to 60,000 |
   | `diagnostics` | Optional diagnostics file (below) |

   The older version 1 shape, `{"version": 1, "host", "port", "secretFile"}`,
   still loads, without realtime or diagnostics.
3. The secret file: at least 32 characters, no whitespace (one trailing newline
   is allowed).

The reference, the configuration and the secret must each be a regular file,
not a link, owned by you and readable by you alone. If anything is missing,
malformed, linked, not loopback, or the secret is weak, the script doesn't
start, and Live's log (Log.txt) has the reason.

### Owner-only on each system

- **macOS and Linux:** the file's owner is your user, and its mode gives group
  and others nothing (for example 600).
- **Windows:** the file's owner is your user, and its access list is protected
  (no inherited entries) with exactly one entry: your user, allowed full
  control. The script checks this by running Windows PowerShell
  (`powershell.exe`, found on Live's PATH, with no console window, up to 10
  seconds a check). Live's own Python has no `ctypes`, so the access-list check
  decides alone there; mode bits aren't checked on Windows. Each Live start
  runs a check for every file involved.

`kumi bridge` and the lifecycle set these permissions themselves.

## How it runs inside Live

Live's embedded Python starves background threads, so the script does its work
on Live's main thread, in the Control Surface's display callback (about ten
times a second) and a scheduled callback. Each tick it:

1. serves the TCP connections for at most 50 ms: reads requests, runs them and
   writes the answers, serving every connection at least one request, and
   waiting up to 12 ms for a client's next request so a change's steps share a
   tick;
2. runs work queued from other threads (realtime writes);
3. advances an audio capture (its watchdog), closes an expired Live undo step,
   lets go of fire buttons held past 30 seconds, and ages its cached view of the
   Set's track and scene structure.

A read stops after about 30 ms of work and returns a cursor for the rest, so no
request holds Live's interface for long. Only two other threads exist: the
realtime UDP receiver, which decodes and checks packets and queues writes for
the main thread, and the diagnostics writer. Neither touches Live's objects.

When Live closes the script (Live quits, or you deselect the Control Surface),
it closes the sockets, stops realtime and any capture, closes an open Live undo
step, lets go of fire buttons, removes its listeners, drops its references and
stops the Willington providers.

## Wire protocol

Newline-delimited JSON over TCP on the configured loopback port, protocol
`ableton-loopback/v1`. Up to 64 connections at a time.

- **Hello.** On connect, the script sends a signed hello with the Live protocol
  (`ableton-live/v1`), the registry's hash and the longest deadline it accepts
  (60 seconds). It carries a bridge epoch, new each time the script starts, and
  a challenge, new for each connection.
- **Requests** carry `version`, `id`, `method`, `nonce` (16–256 characters), a
  `sequence` that rises with every request on the connection, the
  `bridgeEpoch` and `connectionChallenge` from the hello, an absolute
  `deadlineMs` (no more than 60 seconds ahead), and `mac`.
- **Signing.** `mac` is HMAC-SHA256 with the shared secret over the request's
  canonical JSON without `mac` (keys sorted, no spaces, numbers written as
  JavaScript writes them), in unpadded base64url. Answers and events are signed
  the same way and carry the same epoch and challenge, so a captured frame works
  on no other connection and after no restart.
- **Methods.** `status`, `snapshot`, `discover`, `get`, `invoke` (reads and a
  few operations that need no authority), `mutate` (one change with its
  transaction id, idempotency key and optional state digest), `subscribe`,
  `reconnect` (new references, new epoch), and `retire` (forget a transaction's
  recorded results). `preflight`, `prepare` and an authorized `invoke` are a
  three-step form of `mutate` the tests use.
- **Checks.** Every request and result is validated against the registry; an
  `args` object with more than 64 keys is refused. A bad signature, replayed
  sequence, wrong epoch or expired deadline is refused. An answer that breaks
  the registry's contract closes the connection.
- **Errors** are one line of at most 200 characters: the bridge's own reason, or
  the type and text of Live's exception. A refusal made before anything changed
  ends "; nothing changed".
- **Changes.** The script records each change's result under its idempotency
  key (up to 65,536) and returns that result to a retry instead of applying
  twice. `reconnect` and shutdown clear the record.
- **Limits.** A frame up to 256 MiB; a connection whose unsent answers pass
  1 GiB is closed.

Which operations exist, and what each takes, is in the registry
(protocol/ableton-live-v1.operations.json in the repository). The script offers
an operation only when the Live it runs in has what it needs; `status` lists
them. The safety rules every change follows are in docs/en/LIVE_SAFETY.md:
https://github.com/user1303836/kumi/blob/main/docs/en/LIVE_SAFETY.md

### Provenance

Built inside Live, the script reports `real-live` provenance. Built any other
way (tests, a fake Live), it reports `fake-live`. Even `real-live` isn't proof
of a real-Live test without evidence you can see.

## What it can do

The registry holds about 200 operations: reading the Set, changing tracks,
scenes, clips, notes, devices, racks, the mixer, routing, automation and the
transport, recording, deleting, Live undo steps, data stored in the Set, Live
messages, the optional Willington edits, and `python.run`, which runs Python
inside Live with Live's API (docs/en/LIVE_SAFETY.md, "Python in Live").

Two optional parts have their own docs:

- **Realtime control** over UDP (on when `realtimePort` is set):
  https://github.com/user1303836/kumi/blob/main/docs/en/REALTIME_CONTROL.md
- **Audio capture** of Live's Resampling input for analysis:
  https://github.com/user1303836/kumi/blob/main/docs/en/AUDIO_INTELLIGENCE.md

## Diagnostics file

Off by default. `ableton-mcp-lifecycle install --enable-bridge-diagnostics`
creates `bridge-diagnostics.log` in the lifecycle's state folder, owner-only,
and adds it to the configuration. Installing again without the flag turns it
off. Creating a file yourself turns nothing on.

The file records only fixed events (`dispatch-failure`,
`result-contract-failure`, `capture-tick-failure`, `realtime-packet-failure`,
`bridge-accept-failure`), each as one JSON line of at most 512 bytes:

```json
{"category":"validation-error","dropped":0,"event":"dispatch-failure","timeMs":1790000000000,"version":1}
```

`category` is `timeout-error`, `io-error`, `validation-error`,
`internal-error` or `unknown-error`. Nothing else is written: no messages,
tracebacks, requests, names, queries, secrets, tokens, signatures, audio or
paths. Events wait in a queue of 64; when it's full they're dropped and
counted. The file empties itself before it would pass 16 MiB (256 KiB for a
configuration written by an older lifecycle). If the file or its folder changes
identity or permissions, or a write fails, logging stops and the bridge carries
on.

Other messages, such as why the script didn't start or what the Willington
providers did, go to Live's own log (Log.txt). `kumi report` collects the
bridge's lines from it.

## Willington

`willington.json` beside `__init__.py` loads the optional, separately installed
providers. Without the file, nothing changes. Configuration and ownership:
https://github.com/user1303836/kumi/blob/main/docs/en/WILLINGTON_INTEGRATION.md

With the Willington multi-version bundle, install `WillingtonRuntime` beside the
provider packages. At startup their `install()` functions select bindings using
the connected Live process's OS, architecture, version and executable hash.
Validated macOS ARM64 12.4.15b4/b5 bindings can coexist in one installation;
a missing validated component profile skips only that component. Artifact or
integrity failures tear down all native providers; ordinary Kumi stays active.
The existing owner-only `willington.json` opt-in and write controls still apply.
Follow Action evidence must match the selected library, so rerun its self-test
after switching builds, following the [standalone self-test procedure](https://github.com/user1303836/kumi/blob/main/docs/en/WILLINGTON_INTEGRATION.md#follow-action-self-test).
Windows and Intel macOS bindings are not yet available.

The optional Willington rack-zone adapter adds `selector-zone`, `key-zone`, and
`velocity-zone` to `live_willington_device_preview`. Use `ref` for the rack and
`targetRef` for a regular chain. Preview captures all four integer endpoints;
apply and history undo fence rack/chain identity and the complete zone state.
Audio Effect Racks support selector zones; Instrument and MIDI Effect Racks
also support key and velocity zones. Drum/return chains are rejected. Playback
must be stopped. Moving boundaries may require specifying both fade endpoints
to maintain `minimum <= fadeMinimum <= fadeMaximum <= maximum`.

Install the exact-build `WillingtonRackZones` Remote Script package alongside
the existing adapters, then add the optional `"rackZones": true` field to the
owner-only `willington.json`. The existing `enableWrites` flag controls writes.
Rack Zones is supported on Live 12.4.15b5 macOS ARM64 and included in the
validated multi-version bundle. Default installation selects its exact b5 profile;
other builds remain unavailable. The current validation adds 42 actual signal-gating
checks, 49 fade measurements and seven actual Max `live.object` write/read/restore
checks to the earlier Kumi transaction tests. See the
[public validation summary and receipt digests](https://github.com/user1303836/kumi/blob/main/docs/evidence/rack-zones-b5.json) for the measured scope.

## Tests

From the repository, with Python 3.11 (Live 12's version):

```sh
cd remote-script
python -m unittest discover -s . -p 'test_*.py'
```

On Windows, `py -3.11` works in place of `python`. These tests run against a
fake Live; they don't prove a real Live version, a Control Surface installed in
Live, or what a Set does.
