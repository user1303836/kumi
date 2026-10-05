# Ableton MCP Beyond

The Ableton Live bridge behind Kumi, also usable on its own. It is a local MCP
server, `ableton-mcp-server`, plus a Remote Script that runs inside
Live 12. Through it, any MCP client can:

- read the open Set;
- change almost anything in it, each change previewed, then applied, then undone
  when needed;
- play and record;
- analyze audio.

On Live 12.4 and later it also reaches Kumi's Live extension, for offline
renders and MIDI written straight into the Arrangement.

Started without a configuration, the server never connects to Live.

## Using Kumi?

Run `kumi bridge` with Live closed, then pick **AbletonMcpBridge** as a Control
Surface in Live. Kumi installs, starts and updates the bridge for you; see
[Get started](https://github.com/user1303836/kumi/blob/main/README.md#get-started).

Kumi starts the server itself and allows only the tools it uses. Its model calls
the read tools directly. Kumi's own tools make the changes, from tempo and the
mixer to clips, devices, deletions and recording, through previews and
applies. They also render audio and run Python inside Live. See
[how Kumi changes your Set](https://github.com/user1303836/kumi/blob/main/docs/en/KUMI_CHANGES.md).

## Quick start from source

Build the native server and worker with Rust and Cargo, from the repository root:

```sh
cargo build --release --locked -p ableton-mcp-server --bins
./target/release/ableton-mcp-server --version
./target/release/ableton-mcp-server  # offline MCP tools; no Live connection
```

For a configured connection, follow [delivery](https://github.com/user1303836/kumi/blob/main/docs/en/DELIVERY.md), then run:

```sh
./target/release/ableton-mcp-server --config /absolute/path/bridge-config.json
./target/release/ableton-mcp-server diagnostics --config /absolute/path/bridge-config.json
```

Native packages include the server, its sibling analysis worker, the Remote Script and the Live
extension. They need no Node runtime.

## What's in the package

| Command | What it does |
| --- | --- |
| `ableton-mcp-server` | The MCP server on stdio: no arguments, or `--config PATH` |
| `ableton-mcp-server setup` | Writes a server configuration |
| `ableton-mcp-server install-remote-script` | Copies the Remote Script into Live's Remote Scripts folder |
| `ableton-mcp-server diagnostics` | Checks the native runtime, the package, the configuration and the connection to Live |
| `ableton-mcp-server lifecycle` | Installs, activates, upgrades, repairs, rolls back and uninstalls a release |
| `ableton-mcp-server migrate` | Converts an older configuration file |

## Before you connect a client

Every change has a preview and an apply, and most have an undo. A confirmation
the server hands out is not a person's approval. A client should show the
preview before anything that plays, records or deletes.

Limit what a client can do with `ABLETON_MCP_TOOL_POLICY` (`read-only`,
`edit-no-audio`, `performance` or `full`). `full` includes `live_run_python`,
which runs any Python inside Live. Deny it with
`ABLETON_MCP_TOOL_DENY=live_run_python` for a client you don't fully trust.

## Documentation

- [User guide](https://github.com/user1303836/kumi/blob/main/docs/en/USER_GUIDE.md):
  setup, configuration, policy and every tool.
- [Worked examples](https://github.com/user1303836/kumi/blob/main/docs/en/USER_JOURNEYS.md).
- [Live safety](https://github.com/user1303836/kumi/blob/main/docs/en/LIVE_SAFETY.md).
- [Operations](https://github.com/user1303836/kumi/blob/main/docs/en/OPERATIONS.md) and
  [recovery](https://github.com/user1303836/kumi/blob/main/docs/en/RECOVERY.md).
- [Installing a release](https://github.com/user1303836/kumi/blob/main/docs/en/DELIVERY.md).
- [Capabilities](https://github.com/user1303836/kumi/blob/main/docs/en/CAPABILITY_MATRIX.md)
  and [supported platforms](https://github.com/user1303836/kumi/blob/main/docs/en/SUPPORT_MATRIX.md).

Every guide is also in Japanese and Simplified Chinese:
[日本語](https://github.com/user1303836/kumi/blob/main/docs/ja/USER_GUIDE.md) ·
[简体中文](https://github.com/user1303836/kumi/blob/main/docs/zh-CN/USER_GUIDE.md).

A release tarball carries the same guides under `release-docs/`, matching its
version; prefer those when they differ from `main`. The bridge builds and tests
with Cargo on its own, and doesn't need Kumi installed.

MIT licensed. Release tarballs are unsigned. Ableton Live is a trademark of
Ableton AG; this project is not affiliated with or endorsed by Ableton.
