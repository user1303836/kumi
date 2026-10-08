<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a> · <a href="README.ja.md">日本語</a>
</p>

**A studio producer agent for Ableton Live that learns how you work.** Kumi will literally do anything in your set that Ableton exposes via Extensions/LOM... and even some things it doesn't intentionally expose via the API. From the tedious jobs to the ones you never have time for: rebuilding a sound from a YouTube tutorial, matching your mix to a reference, writing a Max for Live device you describe, or reworking the rack you point at. Every change shows up in HISTORY, most with their own undo, and a whole plan is one undo in Live (Cmd-Z, or Ctrl-Z on Windows). Kumi remembers the techniques you keep, so it fits you better with every session. Support for other DAWs is planned.

https://github.com/user-attachments/assets/f53fe0be-c9a9-476e-a1d5-efe3dc429526

## Quickstart

Needs Ableton Live 12 on macOS 13+ or Windows 10/11.

**macOS** (Terminal):

```sh
curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/main/install.sh | sh
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/user1303836/kumi/main/install.ps1 | iex
```

Kumi opens once it's installed and walks you through signing in and connecting to Live. After that, just run `kumi`. Something off? `kumi doctor` says what to fix.

[Guide](docs/en/KUMI_GUIDE.md) · [Commands, keys and screens](docs/en/KUMI_TUI.md) · [How Kumi changes your Set](docs/en/KUMI_CHANGES.md) · [Changelog](CHANGELOG.md)

## What it does

- **Changes almost anything in the Set:** tempo, scale and groove; the mixer, routing and sidechains; tracks, scenes and clips; notes and MIDI transforms; devices, racks and their parameters.
- **Listens:** loudness, tonal balance, width, tempo and key of a mix, a sample or its own bounce, and how your mix compares with a reference.
- **Judges its own changes:** brings each change back to the level it belongs at (with the last Live Limiter, else a Utility it adds), hears before and after, and keeps a change only if its target improved and nothing else got worse. `/loop` works in rounds until a goal is met; `/goal` keeps at one across turns.
- **Measures a style:** a file, a folder, a YouTube or Spotify link, or words like "Burial" or "dub techno" become example tracks and a measured profile with the style's range, kept for next time.
- **Matches a reference:** builds several takes on a sound, scores each against the reference and refines the best. `/loop` keeps at it until it gets there.
- **Watches tutorials** from YouTube or a file, then builds what they show on a new track.
- **Plays, records and resamples**, to let you hear what it built or to check its work.
- **Takes full control of Live:** deletes what you ask for, writes MIDI straight into the Arrangement, renders offline, runs Python inside Live for what its other tools can't reach, and answers for what you right-click in Live ("Ask Kumi about this"). Big Sets of hundreds of tracks stay quick.
- **Makes Max for Live devices** you describe in plain words (MIDI effects, audio effects and instruments) and puts them on your tracks.
- **Looks things up:** searches the web and reads pages, PDFs, manuals and code on GitHub, so it can build an effect like one it has read about.
- **Shows where you are:** FOCUS follows what you touch in Live, as a device tree, a piano roll or a Session or Arrangement strip. Click a device to point at it: "this Saturator's too harsh".
- **Remembers:** notes about you and each Set, techniques it learns from what you keep, and recipes you can replay. Every save is shown, and one click forgets it.
- **Keeps your conversations** for each Set, and says what changed while it was closed.
- **Works with your model:** sign in with ChatGPT, or use an OpenAI, Anthropic or OpenCode API key.

## Status

- Kumi 1.11.2 is currently supported for Ableton Live 12.4.15b4 (beta) and above on macOS and Windows. You may run into issues on lower versions. If you do, **please file an issue!**
- 1.11.2 is tested with Live on macOS. On Windows, installing and updating are tested; using it with Live there is still new. Please send a `kumi report` when something breaks.
- Support for Reaper and Renoise are planned

## Development

Build this checkout with Rust and Cargo:

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --                     # add bridge, doctor, or other arguments after --
npm ci --prefix crates/kumi-runtime/tests/support  # once: the official SDKs some tests run
sh scripts/test-isolated.sh                        # no Live or sign-in needed
```

Existing `npm run setup` and `npm run kumi -- ...` commands remain available. With Cargo installed,
these build and run this checkout. Without Cargo, they install and launch the matching published
native release, keeping the same settings, sign-ins, conversations and library in `~/.kumi`.
After that handoff, `kumi` runs the native application directly.

| Folder | What's in it |
| --- | --- |
| `crates/kumi` | The native terminal app and `kumi` command |
| `crates/kumi-runtime` | Model providers, memory, audio analysis, video, the web, and the Live integration |
| `crates/ableton-mcp-server` | The native bridge, also usable by other MCP clients ([bridge guide](crates/ableton-mcp-server/README.md)) |
| `remote-script` | The bridge's Remote Script, which runs inside Live |
| `apps/live-extension` | Kumi's Live extension (Live 12.4 and later) |
| `protocol` | The list of operations the bridge and the Remote Script share |

The [developer guide](docs/en/DEVELOPER_GUIDE.md) covers building, testing and releasing.

## License

[MIT](LICENSE.md); the parts from other people's work, and the models Kumi fetches, are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Ableton Live is a trademark of Ableton AG; Kumi is not affiliated with or endorsed by Ableton.
