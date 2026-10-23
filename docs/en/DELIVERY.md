# Installing the bridge

English · [简体中文](../zh-CN/DELIVERY.md) · [日本語](../ja/DELIVERY.md)

The bridge has two parts: the `AbletonMcpBridge` Remote Script that Live
loads, and a local MCP server that Kumi (or another MCP client) starts. Both
come in one package, `@ableton-mcp/mcp-server`, and one tool installs them: the
bridge's lifecycle CLI, `ableton-mcp-server lifecycle`. It plans before it changes
anything, records what it installed in a receipt, and can repair, roll back and
remove exactly that. With Kumi, `kumi bridge` runs it for you.

Kumi's bridge for Logic Pro is another program, which Kumi sets up in its own
folder rather than in Logic Pro: see [Logic Pro's bridge](#logic-pros-bridge).

## With Kumi

Run `kumi`: it puts the bridge in Live and can restart Live for you. By hand, quit Live, then run
`kumi bridge`. It:

1. refuses while Live is running, and asks you to confirm Live is closed
   (`--yes` confirms beforehand);
2. copies the bridge's package from the Kumi bundle (in a checkout, packs it
   with `scripts/build-native-release.py --bridge-only` instead) into a folder of its own and checks its hash;
3. runs the lifecycle's `install`, or `upgrade` when a bridge is already there:
   first a plan, then the change;
4. puts Kumi's Live extension into Live's Extensions folder, which Live 12.4
   and later run;
5. waits up to ten minutes for Live to connect through the new bridge, running
   the lifecycle's `activate` every few seconds.

The first time you open Live afterwards, choose **AbletonMcpBridge** as a
Control Surface in **Settings → Link, Tempo & MIDI**. In a checkout with
uncommitted changes, `kumi bridge --allow-dirty` installs anyway (developers
only).

`kumi update` runs `kumi bridge` when the bridge needs an update and Live is closed. The first
native app startup also migrates a legacy JavaScript bridge at the same bridge version. Its
receipt-bound configuration, secret and ports are preserved. `kumi uninstall` offers to take the bridge and the
extension out of Live through the lifecycle's `uninstall`, and keeps the
bridge's files while Live still loads from them. `kumi doctor` checks the whole
chain. The [Kumi guide](KUMI_GUIDE.md#connect-to-live) covers this from a
producer's side.

| What | Where |
| --- | --- |
| The bridge's package | `~/.kumi/bridge/<version>-<time>/package` (older Node generations retain their original `node_modules` layout) |
| Its state: secret, configuration, receipt, journal | `~/.kumi/bridge/state`, or the existing owner receipt’s state folder |
| The Remote Script | `AbletonMcpBridge` in your User Library's Remote Scripts folder (see [Live's folders](#lives-folders)) |
| Kumi's Live extension | `kumi.kumi` in Live's Extensions folder |

`KUMI_REMOTE_SCRIPTS_DIR` and `KUMI_LIVE_EXTENSIONS_DIR` override the two Live
folders, `KUMI_HOME` moves `~/.kumi`, and `KUMI_BRIDGE_WAIT_SECONDS` sets how
long to wait for Live (`0` doesn't wait). Kumi finds the installed bridge
through `bridge-reference.json`, which the lifecycle writes beside the Remote
Script.

## REAPER's extension and Renoise's tool

Kumi's bridges in REAPER and Renoise go in through a lifecycle of Kumi's own
(`crates/daw-bridge`), not the bridge's CLI. Setup, `kumi --daw reaper bridge`
(or `renoise`) and `kumi update` run it; the
[Kumi guide](KUMI_GUIDE.md#connect-to-reaper-or-renoise) covers this from a
producer's side.

- **In the bundle:** `daw-mcp-server` beside `kumi`, and a `daws` folder:
  `daws/reaper/reaper_kumi.dylib` (`.dll` on Windows, `.so` on Linux) with
  `reaper_kumi.version`, and `daws/renoise/com.kumi.Bridge.xrnx/`. The bundle's
  `package.json` and `kumi-install.json`, and the release's `kumi-release.json`,
  give their versions as `reaper` and `renoise`; `bridge` stays Live's.
- **Into the DAW:** REAPER's extension into the `UserPlugins` folder of the
  REAPER Kumi found (a portable one, or the folder `KUMI_REAPER_RESOURCE_DIR`
  names), with `kumi-bridge-reference.json` beside it; Renoise's tool into
  `Scripts/Tools` in its newest preferences folder from 3.5, with
  `bridge-reference.json` inside. Updates and rollbacks keep each where Kumi put
  it and bring along every copy of Kumi's the receipt lists (`copies`), such as
  one a newer Renoise made in its own version folder; uninstall takes each of
  those out too. A bridge the producer took out isn't put back. Each is copied
  under another name and renamed into place, never written over where it is (a
  running REAPER keeps the old file; Windows locks it). On macOS, Kumi's copies
  lose the download mark.
- **Receipt and generations:** `~/.kumi/bridges/<daw>/install-receipt.json`
  records each step, the newest 20: `install`, `upgrade`, `reinstall` (the same
  copy over one that changed), `rollback` and `uninstall`. The copy in place and
  the three before it are kept in `generations` for rollback; an update that
  brings the copy already in place changes nothing.
- **Nothing lost:** a copy in the DAW's folder that isn't one Kumi put there
  (changed since, or there before Kumi) is kept in `~/.kumi/bridges/<daw>/changed`
  before anything replaces or removes it. The Renoise tool's `preferences.xml`,
  which Renoise writes, stays with the tool, and `.DS_Store`, `Thumbs.db` and
  `desktop.ini` never count as a change. `kumi uninstall` leaves those copies
  where they are, and catch-up's baselines (`snapshots`) unless `--all`; the rest
  of `~/.kumi/bridges` goes.
- **Rollback and removal:** `kumi update --rollback` takes each bridge to the
  copy the earlier Kumi carries (a Kumi that knows neither DAW leaves both where
  they are); `kumi uninstall` takes them out where the receipt says they went.

## Logic Pro's bridge

Logic Pro loads nothing from outside, so its bridge doesn't go into Logic Pro.
It's a program of Kumi's, `logic-kumi`, with its Accessibility helper,
`kumi-logic-hands-<hash>` (a universal, ad hoc signed Swift program), and Kumi's
host for Logic Pro, `daw-mcp-server --daw logic`, which Kumi starts from beside
its own binary, starts it beside Logic Pro when a session begins. Only the
Apple Silicon Mac's Kumi carries it, in `daws/logic/logic-kumi/` beside `kumi`:
the bridge, the helper and a `version` file.

`kumi --daw logic bridge` (or setup's **Connect to Logic Pro**) copies that
folder into Kumi's own folder through the lifecycle Kumi's own bridges share
(daw-bridge's): it checks the folder's hash, records a receipt, keeps the
earlier versions for rollback, and refuses links in it. Logic Pro can stay
open: nothing of Logic Pro's changes, and the bridge only runs while Kumi does.

| What | Where |
| --- | --- |
| The bridge, its helper and `version` | `~/.kumi/bridges/logic/bridge/logic-kumi/`, with `bridge-reference.json` |
| Its secret, discovery files and log | `bridge.secret`, `discovery/` and `logs/` in `~/.kumi/bridges/logic/` |
| The receipt, the lock, the journal of a change under way, and the versions kept for rollback | `install-receipt.json`, `lifecycle.lock`, `lifecycle-journal.json` and `generations/` in `~/.kumi/bridges/logic/` |

The bridge listens on 127.0.0.1, on the first free port from 9790 to 9799, and
says where in its discovery file; Kumi's host and the bridge prove they know
its secret before anything else passes. `kumi update` brings the bridge up to
date when the new Kumi carries a newer one, `kumi update --rollback` puts back
the earlier Kumi's, and `kumi uninstall` takes it out, none of them waiting on
Logic Pro. `kumi --daw logic doctor` checks it. On an Intel Mac, Linux or
Windows, Kumi carries no Logic Pro bridge, and `kumi --daw logic bridge` sets
nothing up, saying Logic Pro is supported on Apple Silicon Macs only.

`KUMI_LOGIC_BRIDGE_SOURCE` names a folder laid out as a release lays the bridge
out, to set it up from instead. A checkout's build on a Mac stages its own
(`cargo build -p logic-kumi`, and the helper `scripts/build-logic-hands.py`
builds when it isn't there yet) in `target/logic-bridge/logic-kumi/` and sets
that up.

## The standalone bridge

For MCP clients other than Kumi, use the native archive for your platform. No separate Node
runtime is required. Build one from a clean checkout with:

```sh
python3 scripts/build-native-release.py --bridge-only --out release/bridge
```

The output includes a target-specific `.tar.gz` and `prepared.json` with its exact SHA-256.
Packages built from uncommitted changes require `--allow-dirty-private-build`.

Extract the archive into a permanent directory. Keep `ableton-mcp-server` and
`ableton-mcp-analysis-worker` together with the package's assets and release manifest.
For example, on macOS:

```sh
ARTIFACT=/absolute/path/to/ableton-mcp-server-1.0.74-aarch64-apple-darwin.tar.gz
ARTIFACT_SHA="$(shasum -a 256 "$ARTIFACT" | awk '{print $1}')"
INSTALL_ROOT="$HOME/Library/Application Support/AbletonMcp/package"
STATE="$HOME/Library/Application Support/AbletonMcp/state"
REMOTE_SCRIPTS="$HOME/Music/Ableton/User Library/Remote Scripts"
mkdir -p "$INSTALL_ROOT" "$REMOTE_SCRIPTS"
tar -xzf "$ARTIFACT" -C "$INSTALL_ROOT"
PACKAGE_ROOT="$INSTALL_ROOT/package"
SERVER="$PACKAGE_ROOT/ableton-mcp-server"

"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA"
# Read the plan and quit Live, then:
"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA" \
  --apply --confirm-live-stopped
```

On Windows, extract the matching archive with `tar -xzf`, then invoke
`& "$PackageRoot\ableton-mcp-server.exe" lifecycle install` with the same options, using your
absolute Windows paths. `Get-FileHash -Algorithm SHA256 $Artifact` gives the archive's hash.
Use your User Library's Remote Scripts folder (see [Live's folders](#lives-folders)).

Open Live, choose **AbletonMcpBridge** as a Control Surface, then run `activate` with the same
`--remote-scripts-dir`, `--state-dir` and `--package-root`. Point the MCP client at the installed
server with `--config <state>/bridge-config.json`; see the [user guide](USER_GUIDE.md).

For an upgrade, extract the new tarball into a new directory and run `upgrade` with its package
root, artifact and hash, retaining the existing state/config/secret paths. A verified legacy Node
installation may migrate to a native package with the same bridge version. Other upgrades must
increase the version. Keep the previous package for rollback. Run `uninstall` before deleting
package directories. The lifecycle still accepts legacy Node release artifacts and receipts;
those require Node 22 or 24.

## Lifecycle CLI reference

```text
ableton-mcp-server lifecycle <action> --remote-scripts-dir DIR [options]
```

| Action | What it does | Needs |
| --- | --- | --- |
| `install` | Creates an owner-only secret and bridge configuration, installs the Remote Script, writes the receipt | `--artifact`, `--artifact-sha256`; Live stopped |
| `activate` | Checks, without changing Live or the installation, that Live loaded this bridge and answers through it; records the result in the receipt | Live running, the Control Surface chosen |
| `upgrade` | Replaces the bridge with a newer package, keeping the secret and the previous version for `rollback` | A newer artifact, or verified same-version Node → native migration; its SHA-256 and package root; Live stopped |
| `repair` | Compares what's installed with the receipt; with `--apply`, moves changed files to quarantine and restores the package's own | — |
| `rollback` | Goes back to the version the last upgrade kept | Live stopped |
| `uninstall` | Removes the files the receipt owns; moves changed or unknown ones to quarantine; keeps the secret | Live stopped |
| `status` | Read-only report: receipt, file integrity, drift, permissions, whether rollback is possible | — |

| Option | Meaning |
| --- | --- |
| `--remote-scripts-dir DIR` | Live's Remote Scripts folder (required) |
| `--state-dir DIR` | Where the secret, configuration, receipt and journal live. Default `~/.config/ableton-mcp`, or `%APPDATA%\ableton-mcp` on Windows |
| `--package-root DIR` | The installed package to use. Default: the package this CLI belongs to |
| `--artifact FILE`, `--artifact-sha256 HEX` | The tarball, and its hash; the lifecycle checks the installed package against the tarball's own manifest |
| `--config FILE`, `--secret FILE` | Other paths for the configuration and secret (defaults: `bridge-config.json` and `bridge.secret` in the state folder) |
| `--host`, `--port`, `--realtime-port` | Loopback address and ports for a new installation: `127.0.0.1` (or `::1`), 9765 and 9766 by default |
| `--timeout-ms N` | The bridge's request timeout written to the configuration (default 5000) |
| `--apply` | Make the change. Without it, every action only plans |
| `--confirm-live-stopped` | You quit Live; needed by `install`, `upgrade`, `rollback` and `uninstall` with `--apply` |
| `--purge-secret` | With `uninstall`: also delete the secret, only if the lifecycle created it |
| `--enable-bridge-diagnostics` | With `install`: turn on the Remote Script's diagnostics log |
| `--allow-dirty-private-build` | Accept a package built from uncommitted changes (developers only) |

Each run prints one JSON result on stdout (`ableton-mcp-lifecycle/v1`) whose
`state` is `planned`, `completed`, `activation-required`, `blocked` or
`failed`. A refusal prints `ableton-mcp-lifecycle-error/v1` on stderr instead,
with paths removed. Blocked, failed and refused runs exit with 2. The receipt's
status is `installed-restart-required` after install, upgrade, repair and
rollback, `activated` once `activate` reached Live through the bridge, and
`uninstalled` after removal.

The lifecycle never quits or starts Live, never chooses a Control Surface,
never guesses Live's folders, and never follows a symlink or junction in the
paths it's given. It holds a lock while it works and keeps a journal of the
last change; a failure part-way puts back what was there. After an interrupted
run, read `status` and the journal before trying again, and use `repair` or
`rollback` as they indicate.

More about each action:

- **Install** checks the tarball's bytes against its hash and the package
  against the tarball's manifest, and that the ports are free, before it
  changes anything. It puts an empty file named `__pycache__` in the Remote
  Script's folder, so Live can't write or load compiled copies of it; anything
  else in that place counts as drift.
- **Activate** records `activated` only after an authenticated answer from the
  real Live with the expected registry hash. A simulator, a stale or wrong
  registry, or no answer gives `activation-required` and says what to do next.
  A recorded activation is history, not proof Live is connected now.
- **Upgrade** needs a strictly newer version and refuses drifted files. It
  keeps the previous version and configuration for `rollback`.
- **Repair** never creates a missing secret, because a new secret would be new
  authority over the bridge. Run it again and it changes nothing.
- **Uninstall** keeps the secret unless `--purge-secret`, and keeps the
  diagnostics log. Deleting is an ordinary unlink, not a secure erase.

**Diagnostics log.** With `--enable-bridge-diagnostics` at install, the Remote
Script writes short, redacted records to `bridge-diagnostics.log` in the state
folder: owner-only, queued and written in the background, at most 16 MiB.
Without the flag there's no log.

## Live's folders

| Folder | macOS | Windows |
| --- | --- | --- |
| Remote Scripts (default User Library) | `~/Music/Ableton/User Library/Remote Scripts` | `Documents\Ableton\User Library\Remote Scripts` (or under `OneDrive\Documents`) |
| Extensions (Live 12.4 or later) | `~/Library/Application Support/Ableton/Extensions` | `%LOCALAPPDATA%\Ableton\Extensions` (not yet confirmed) |
| Control Surface setting | Live → Settings → Link, Tempo & MIDI | Options → Settings → Link, Tempo & MIDI |

If you moved your User Library, Live's **Settings → Library** shows where it
is; Kumi finds it by itself from Live's preferences. Never install into Live's
application folder.

## Checking an installation

```sh
ableton-mcp-server diagnostics --config /absolute/path/to/bridge-config.json
```

It prints a JSON report. It exits with 1 for an unsupported system, and
with 0 even when Live isn't reachable, so read its fields:

| Field | Means |
| --- | --- |
| `runtime`, `runtimeVersion`, `runtimeSupported`, `platformSupported` | Native Rust runtime, bridge version and platform support |
| `readiness.package` | The package and its Remote Script files are present and intact (not that Live loaded them; `status` checks the installed copy) |
| `readiness.configured` | The configuration is valid, names a bridge and has a readable secret |
| `readiness.authenticatedBridge` | The Remote Script answered over the authenticated connection, and discovery worked (`registryHash` shows its registry) |
| `readiness.realLiveOperational` | That answer came from real Live (`real-live` provenance), not a simulator |
| `ready` | All of the above |

The secret is never printed. Reinstalling isn't a way to fix a connection:
check that Live was restarted, the Control Surface is chosen, and the
configuration, secret and ports match.

## Moving a configuration to version 2

`ableton-mcp-server migrate` keeps an old (legacy or version-1) client configuration
as it is by default. Given every bridge field and an existing owner-only secret,
it writes a version-2 bridge configuration:

```sh
ableton-mcp-server migrate --input /absolute/old.json --output /absolute/bridge-v2.json \
  --bridge-host 127.0.0.1 --bridge-port 9765 --realtime-port 9766 \
  --secret-file /absolute/bridge.secret
```

It never creates a secret, accepts only loopback hosts, and refuses to replace
an existing file without `--force`.
