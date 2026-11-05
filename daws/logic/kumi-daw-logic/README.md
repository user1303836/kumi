# Kumi's Logic Pro adapter: M2, M3

- The app adapter for the original Logic M2 reader. It reads project, track, region, note, plug-in, send, marker and tempo rows through `logic_read` and draws four FOCUS bodies: Channel Strip, Piano Roll, Tracks and Routing.
- Requires Logic Pro 12 on macOS. The bridge attaches to an open Logic; it installs nothing into Logic and requires no Logic restart. Its binary, built Accessibility helper and connection reference go in `KUMI_HOME/bridges/logic/bridge/logic-kumi`.
- Allow the app Kumi runs in under System Settings → Privacy & Security → Accessibility. Setup polls the authenticated bridge's permission status; dialogs and another desktop are reported separately.
- Saved projects open with the attached Logic's Apple bundle id. Kumi reports the saved project, the selected items and the recording state. M3 offers 16 change kinds and three transport actions, with HISTORY and by-value undo. Logic has no atomic undo block; its native undo skips volume, pan, mute, solo, arm, tempo and bypass. Listening, rendering, goals, scripts and catch-up remain disabled; library sources and learning are reserved for M6.
- Release files carry `logic-kumi`, a `kumi-logic-hands-*` helper and `version`. A checkout builds the native bridge and helper explicitly. `KUMI_LOGIC_BRIDGE_SOURCE` may name an existing release-like folder; setup creates no native build.
- Isolated adapter/provider tests use local fixtures and stand-ins for project opening. They launch no real DAW and need no Accessibility grant. Recorded native replay and actual Logic/app acceptance are separate checks.

Native acceptance on a disposable project requires `accept_logic --changes --scratch-confirmed`; failed undo may leave edits. Recorded source evidence is not a current native acceptance result.

Deleted tracks and markers require manual Logic undo; Kumi never presses a native Undo step identified only by its title. Confirmed deletions stay in HISTORY with this limitation; uncertain deletions remain uncertain. Native marker, note and structural undo attribution is not inferred from titles alone.
