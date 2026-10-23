# Realtime control

English · [简体中文](../zh-CN/REALTIME_CONTROL.md) · [日本語](../ja/REALTIME_CONTROL.md)

A UDP endpoint in the Remote Script for fast parameter control from a
controller script, an OSC app or a Max patch, for up to 30 seconds at a time.
It's for MCP clients; Kumi doesn't use it.

An open port grants nothing. Packets are dropped until a client arms the plane
through the bridge, and an arm names the exact parameters packets may move.

## Configuration

The Remote Script listens on UDP when the `bridge` section of its version 2
configuration has a `realtimePort` that differs from `port`:

```json
"bridge": {"host": "127.0.0.1", "port": 9765, "realtimePort": 9766, "secretFile": "/absolute/path/to/bridge.secret", "timeoutMs": 5000}
```

The whole file is described in the Remote Script's README
([daws/ableton/remote-script/README.md](../../daws/ableton/remote-script/README.md)).

- `ableton-mcp-lifecycle install`, and so `kumi bridge`, sets `realtimePort` to
  9766 unless `--realtime-port` names another port. Install checks that both
  ports are free first.
- `ableton-mcp-setup` writes it only when you pass `--realtime-port` with the
  other bridge options ([user guide](USER_GUIDE.md)).
- Without `realtimePort`, there is no UDP socket and the `realtime.*`
  operations aren't offered.

Both sockets bind only the configured loopback address. If the UDP port is
taken when Live starts, the Remote Script doesn't load at all, TCP included:
free the port, or reinstall with another.

## Arming

1. Call `live_realtime_arm_preview` with:
   - `channels`: one to four of `udp-json`, `osc`, `xy` and `max`;
   - `parameterRefs`: 0 to 32 parameter refs from fresh discovery (an empty
     list allows emergency-stop packets only);
   - `ttlMs` (optional): 1,000 to 30,000, default 10,000;
   - `sourcePorts` (optional): up to 16 UDP ports packets may come from;
   - `outputSafety` (optional).

   It needs a connected `real-live` Remote Script that offers `realtime.arm`,
   `realtime.disarm` and `realtime.stats`. The preview records each
   parameter's identity together with its device, its track and the device's
   other parameters.
2. Call `live_realtime_arm_apply` with the transaction id,
   `confirmation: "apply"` and an idempotency key. On Live's thread, the Remote
   Script compares every recorded identity with Live as it is now, and refuses
   if a parameter was replaced or moved.
3. The result holds the endpoint (`host`, `port`), a bearer `token`,
   `expiresAt`, the channels and parameter refs, and the limits. Keep the token
   out of logs and files.
4. Send packets (below), and read `live_realtime_stats` to see what happened to
   them.
5. Call `live_realtime_disarm` with `confirmation: "disarm"` when you're done.

Arming again replaces the token, starts the sequence over and drops writes
still queued from the earlier arm. Disarm, expiry and Remote Script shutdown
drop them too.

## Packets

### UDP JSON

One JSON object per datagram, at most 512 bytes. Unknown fields are refused.

Set one parameter:

```json
{"token":"<arm token>","seq":1,"channel":"udp-json","op":"parameter.set","ref":"<parameter ref>","value":0.5,"sentAtMs":1700000000000}
```

Set two parameters together (if either write fails, both go back):

```json
{"token":"<arm token>","seq":2,"channel":"xy","op":"xy.set","xRef":"<parameter ref>","x":0.4,"yRef":"<parameter ref>","y":0.6,"sentAtMs":1700000000000}
```

Emergency stop:

```json
{"token":"<arm token>","seq":3,"channel":"udp-json","op":"emergency-stop","sentAtMs":1700000000000}
```

- `seq` is a positive integer up to 2^53 − 1 that rises with every packet of
  an arm. A lower or repeated `seq` is a replay and is dropped.
- `sentAtMs` (optional, milliseconds since 1970) lets the stats measure transit
  jitter; without it they measure the jitter between arrivals.
- `channel` must be one of the armed channels, and must suit the operation:

| Operation | Channels |
| --- | --- |
| `parameter.set` | `udp-json`, `osc`, `max` |
| `xy.set` | `xy`, `osc`, `max` |
| `emergency-stop` | any armed channel |

### OSC

OSC bundles and unsupported argument types are refused.

| Address | Arguments |
| --- | --- |
| `/ableton-mcp/parameter` | token (string), seq (int32 or int64), ref (string), value (number), optional sentAtMs (number) |
| `/ableton-mcp/xy` | token, seq, xRef, x, yRef, y, optional sentAtMs |
| `/ableton-mcp/emergency-stop` | token, seq, optional sentAtMs |

Numbers may be int32, int64, float32 or float64.

### Max

A Max patch can send the UDP JSON objects with `"channel": "max"`, through
`udpsend` to the returned endpoint. No Max device ships with the bridge, and
there is no Max handshake. The bridge's `ableton://max-extension` resource
describes the packet contract.

### Emergency stop

An `emergency-stop` packet stops what plays at that moment, on Live's thread:
Session clips, the transport, Session Record and Arrangement Record.
`live_session_emergency_stop` does the same over the authenticated TCP channel,
without a token.

## Limits

| What | Limit |
| --- | --- |
| Packet size | 512 bytes |
| Rate | 64 packets a second sustained, bursts of 16 (a token bucket) |
| Parameters per arm | 32 |
| Sender ports per arm | 16 |
| Arm lifetime | 1 to 30 seconds |
| Wait for Live's thread | 1 second; a write not started by then is dropped |
| Queue | Shared with the Remote Script's main-thread queue (65,536 entries) |

## What happens to a packet

On the UDP thread, which never touches Live, the Remote Script decodes the
packet and checks the token, the sender, the channel, the target parameters,
the sequence and the rate, then queues the write for Live's main thread. A
packet dropped for its rate has already used its `seq`: send the next number,
not the same one again.

On Live's main thread, at the next display tick, it checks that the arm is
still current and that each parameter still has the identity recorded at arm
time; any change revokes the arm. It then checks the value: within the
parameter's range, on one of its steps, and the parameter enabled. Values that
fail are refused, never moved to fit (unlike the typed parameter tools). Then
it writes the value and reads it back.

`accepted` means queued for Live's thread, not written; UDP itself never
confirms delivery. Only `applied` means written and read back.

`live_realtime_stats` reports:

| Field | Meaning |
| --- | --- |
| `armed` | Whether an arm is current |
| `accepted`, `applied`, `pending` | Queued, written and confirmed, and still waiting |
| `applyFailures`, `revokedBeforeApply` | Writes that failed on Live's thread, and those dropped because the arm ended or a target changed |
| `droppedBeforeDispatch`, `droppedQueueFull` | Writes Live's thread didn't start within 1 second, or couldn't queue |
| `droppedUnarmed`, `droppedEndpoint`, `droppedTarget`, `droppedInvalid`, `droppedReplay`, `droppedRateLimited` | Packets dropped with no arm or a wrong token or channel, from the wrong sender, for a parameter not armed, malformed, replayed, or over the rate |
| `sequenceGaps`, `lastSequence` | Missing sequence numbers so far, and the last one accepted |
| `jitterMs`, `maxJitterMs` | Smoothed and largest jitter |

## Recovery

- On any `applyFailures`, `revokedBeforeApply` or `droppedBeforeDispatch`, or
  `pending` that doesn't fall to zero, disarm and discover again before arming
  anew.
- A dropped packet is counted, never retried for you.
- If the MCP host restarts while Live keeps running, the token still works
  until it expires; reconnecting doesn't extend it.
- If Live or the Remote Script restarts, the socket closes and every token is
  gone.
- `live_recovery_finalize` refuses while an arm is current or writes are
  pending.
- After a session, put the parameters you moved back and check that Live is
  stopped and not recording.

## Evidence

[phase-7c-realtime-live.json](../evidence/phase-7c-realtime-live.json)
exercised all four channels, replay, sender and target drops, and parameter
restoration on Live 12.4.5b8 on macOS (2026-07-27, bridge 0.1.0). It predates
the current bridge, and no Windows run is tracked. No Max device has been
tested.
