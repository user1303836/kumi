# Recovery procedures

English · [简体中文](../zh-CN/RECOVERY.md) · [日本語](../ja/RECOVERY.md)

What to do when a call fails or a change is left in doubt.

The rule: **never answer an uncertain change with a new one.** If an apply
times out or loses its reply, retry only that apply, with the same
`transactionId` and `idempotencyKey`, while the server and Live are still up.
Otherwise read Live again and put things right by hand.

## Reading an error

A malformed request gets a JSON-RPC error:

| Code | Meaning |
| --- | --- |
| `-32700` | The line isn't JSON |
| `-32600` | Invalid request, a message over 500 MiB, an `id` already in use, or the server is shutting down |
| `-32601` | No such tool |
| `-32602` | Invalid arguments, or a protocol-version mistake |
| `-32002` | Not initialized yet, or no such resource or prompt (`2025-11-25`) |
| `-32022` | Unsupported protocol version (the error lists the supported ones) |
| `-32000` | Busy: too many requests waiting; retry when some finish |
| `-32603` | Internal error |

A tool that refuses answers with `isError: true` and
`{"reason": "...", "remediation": "..."}`. The reason is the bridge's or Live's
own, in one line. Read it:

- **"Nothing changed in Live"**, or a reason ending "; nothing changed": the
  call did nothing. Fix what it says, then preview again.
- **"Live state changed since the preview"**: something changed between preview
  and apply, perhaps your own edit in Live or another client. Read again, then
  preview again.
- **`tool-unavailable-in-current-live-shape`**: this Live doesn't offer the
  tool right now. Read `ableton://capabilities` to see why.
- **`tool-denied-by-deployment-policy`**: the
  [deployment policy](USER_GUIDE.md#deployment-policy) hides it.
- **The remediation says the change is uncertain**: see the next section.

## When a change is uncertain

A timeout, a disconnect or a lost reply during an apply or an undo leaves the
change uncertain: it may or may not have happened.

1. Don't preview it again, and don't send a new key.
2. While the same server runs and Live hasn't restarted, send the same apply
   (or undo) again with the same `transactionId` and `idempotencyKey`. The
   Remote Script remembers what it ran. It answers with the first result, or
   finishes the change, and the bridge reads it back.
3. If Live restarted meanwhile, the retry is refused. Read the Set
   (`live_discover`, `live_snapshot`), see whether the change is there, and set
   things right by hand.
4. Then close the record with `live_recovery_finalize`:

   ```json
   {"transactionId": "<id>", "resolution": "manually-restored", "confirmation": "finalize-recovery-record",
    "evidence": {"provenance": "checked the mixer in Live", "scope": "track 3 volume"}}
   ```

   Use `"accepted-current-state"` when you keep Live as it is. Finalizing
   changes nothing in Live. It is refused while anything plays, records or
   holds a realtime channel.

Uncertain records count against the server's undo capacity. When they fill it,
new changes are refused ("capacity is exhausted by recovery-protected work")
until you finalize them.

## Common problems

| Problem | What to do |
| --- | --- |
| A JavaScript bridge, installed by Kumi 1.7.5 or earlier, exits with "Unsupported Node.js" | Run it with Node 22 or 24. The native bridge doesn't use Node. |
| "version-1 configuration does not enable a Live adapter" | Write a version 2 file with `ableton-mcp-setup` and the bridge options; see [the configuration file](USER_GUIDE.md#the-configuration-file). |
| "secret file is invalid", or its permissions "must be conclusively owner-only" | The secret must be one line of 32 or more characters, readable only by you. For a lifecycle install, `ableton-mcp-lifecycle repair` restores them. |
| `live_status` says `"connected": false` | Check that Live is running with **AbletonMcpBridge** chosen as a Control Surface, and that the configuration's port and secret are the ones the Remote Script uses. Then run `ableton-mcp-diagnostics --config <path>`. |
| AbletonMcpBridge doesn't load in Live; Live's log says "bridge configuration reference is missing or unsafe" | The Remote Script was installed without `--config`. Install it again with `--config` (see [connect to Live](USER_GUIDE.md#connect-to-live)), or use the lifecycle. |
| "Unknown or expired … transaction" | The preview expired or the server restarted. Preview again. |
| `live_undo` refuses: the ref "isn't the one this change was made on any more" | The object was replaced. Put it right by hand, or use `live_song_undo` if Live's last undo step is that change. |
| You deleted something by mistake | `live_song_undo` (`confirmation: "undo-in-live"`) straight away; `live_undo` can't bring deletions back. |
| The Live extension's tools are missing | The extension isn't running: it needs Live 12.4 or later, and either installing in Live's Extensions folder or Live's Developer Mode; see [Kumi's Live extension](USER_GUIDE.md#kumis-live-extension). |

Installation problems (lifecycle receipts, quarantine, repair and rollback) are
covered in [delivery](DELIVERY.md).

## Stop everything that plays or records

`live_session_emergency_stop` stops Session clips, the transport and both
recording modes. It needs no transaction, so it works after a restart too.

1. Read what's playing: `live_discover` with `{"kind": "session-playback"}`.
2. Send exactly what you read:

   ```json
   {"confirmation": "emergency-stop", "expectedTargets": ["<trackRef>|<clipSlotRef>|<sceneRef>"],
    "expectedRecording": "session"}
   ```

   `expectedRecording` is `stopped`, `session`, `arrangement` or `both`.

3. If it's refused because playback changed meanwhile, read again and repeat.
   Success reports `"stopped": true` and `recordingStopped`.

To stop one clip you launched, use `live_clip_launch_stop`; for an audition,
`live_session_audition_stop`. A capture has its own emergency stop; see
[audio intelligence](AUDIO_INTELLIGENCE.md). For realtime control, call
`live_realtime_disarm` and see [realtime control](REALTIME_CONTROL.md).

## After a restart

The server keeps its undo records and previews in memory only, so a restart
loses them. A Live restart, or a reconnect, gives Live a new epoch, and every
ref from before stops working.

1. Start the server and initialize again.
2. Call `live_status`, and read the Set again.
3. Changes made before the restart can't be undone with `live_undo`. Live's own
   undo (`live_song_undo`) may still hold them.
4. If a change was uncertain when the server stopped, check it by hand: there
   is no record left to retry or finalize.
