# Supported platforms

English · [简体中文](../zh-CN/SUPPORT_MATRIX.md) · [日本語](../ja/SUPPORT_MATRIX.md)

What Kumi and its bridges run on, which versions of Live, REAPER and Renoise
they work with, and what has been tested where. [Implementation status](IMPLEMENTATION_STATUS.md)
lists the evidence behind each "tested".

## Kumi

| System | Versions | Processors | Status |
| --- | --- | --- | --- |
| macOS | 13 (Ventura) or later | Apple silicon, Intel | Native build/install CI; earlier TypeScript releases tested with Live on Apple silicon |
| Windows | 10 or 11 | x64, ARM64 | Native build/install CI; earlier TypeScript releases tested with Live on Windows 10; see [Windows](#windows) |
| Linux | glibc 2.35 or later (Ubuntu 22.04, Debian 12 or newer; not Alpine or other musl) | x64, ARM64 | Native build/install CI; no Live for Linux, but REAPER and Renoise run there |

- Kumi and the standalone bridge run as native Rust programs. Fresh installs do not download or require Node.
- Existing Kumi 1.7.4 and 1.7.5 installations using Node 24 move to native through `kumi update`, keeping the same settings, sign-in and data. The retained Node supports rollback and optional YouTube challenges. On Windows, the first native start replaces the old launcher, so later starts don't go through Node. Older Node majors may need the installer rerun with the same `KUMI_HOME`; see [updating](KUMI_GUIDE.md).
- YouTube challenges use retained Node or Node on PATH; Kumi does not fetch Node for them. Live runs its extension in Live's own JavaScript host.

Real-Live results below describe the earlier TypeScript releases. Native CI and migration checks are separate from acceptance on actual Live hardware.

## Ableton Live

| Live | Status |
| --- | --- |
| 12.4 or later | Everything, including Kumi's Live extension: offline renders, MIDI clips written into the Arrangement, clearing a range, and **Ask Kumi about this**. Tested on Live 12.4.15 beta on macOS. |
| 12.0 to 12.3 | The bridge offers what that Live's API has; no extension, so none of the above. `kumi doctor` says so. Not tested. |
| 11 or earlier | Not supported. |

Editions: the bridge discovers what the Live it connects to offers, so devices
and content an edition lacks (Standard and Intro have fewer) stay unavailable
rather than guessed. Making Max for Live devices needs Max for Live (Suite, or
Standard with the add-on). Willington (in Kumi's bridge from the release that
carries its files, or installed yourself; off until `/willington`) has bindings
for macOS ARM64 Live 12.4.15b4 and b5 (rack chain
zones on b5 only) and Windows x64 Live 12.4.15b5; see
[Willington](WILLINGTON_INTEGRATION.md).

Native editing (groups, Arrangement automation, scene/global Follow Actions and per-note MPE) requires the installed validated `WillingtonEditing` component, a passing exact-library self-test and explicit opt-in. It supports only Live 12.4.15b5 macOS ARM64; not b4, Intel macOS or Windows. See [the self-test procedure](WILLINGTON_INTEGRATION.md#native-editing-and-its-self-test).

## REAPER and Renoise

| DAW | Versions | Systems | Status |
| --- | --- | --- | --- |
| REAPER | 7 or later | macOS (one extension for Apple silicon and Intel), Windows x64 (the same extension for REAPER on Windows on Arm, which loads x64), Linux x64 and ARM64 (glibc 2.35 or later) | Tested with REAPER 7.82 (evaluation, portable) on macOS. CI builds the extension on each system, checks it and loads it with the system's own loader. Not yet tried with REAPER on Windows or Linux; Windows on Arm untested |
| Renoise | 3.5 or later | macOS, Windows, Linux | Tested with Renoise 3.5.4 on macOS. CI runs the tool's tests against a mock of Renoise's API and checks it against Renoise's own API definitions. Not yet tried with Renoise on Windows or Linux |

- Linux is a DAW platform for both: setup connects REAPER and Renoise there as on
  macOS and Windows.
- On every system, CI puts both bridges into scratch DAW folders, updates them,
  rolls back twice and uninstalls them, through Kumi's own commands.
- Renoise's demo can't render, so Kumi can't listen to a song there.

## Windows

Tested: the installer, `kumi update`, `kumi bridge` and `kumi uninstall` in
Windows PowerShell 5.1 and PowerShell 7 on CI, and the installer, `kumi bridge`,
rollback and `kumi uninstall` with `KUMI_HOME` in a folder named in non-ASCII
letters; and on a Windows 10 machine with Live 12.4.15
beta, `kumi bridge` into a User Library moved outside the user folder, the
Remote Script loading in Live, and Kumi connecting. CI's Windows runner is an
administrator account with default folders, so it can't show problems that
only an ordinary account or a moved library has.

Not yet confirmed on Windows:

- **Where Live keeps its Extensions folder.** Kumi uses
  `%LOCALAPPDATA%\Ableton\Extensions`; `KUMI_LIVE_EXTENSIONS_DIR` overrides it. Until
  that's confirmed, the extension's features are untested on Windows.
- **The full-screen app in Windows terminals.** Windows Terminal is recommended;
  see [terminals](KUMI_TUI.md#terminals).
- **`kumi update` from Kumi 1.6.0 or earlier** fails with a tar error when Git's
  `tar` comes before Windows' own on PATH (as in a PowerShell started from Git
  Bash). Run the installer line again, or run
  `$env:Path = "$env:SystemRoot\System32;$env:Path"` before `kumi update`.

## Node.js for older installations

| Node.js | Status |
| --- | --- |
| 22.x, 24.x | Supported; Node 24 LTS recommended |
| 25.x | Not supported: it reached end of life on June 1, 2026 |
| 26.x and later, 21.x and earlier, prereleases | Not supported until tested |

This table applies to installations of Kumi 1.7.5 and earlier, which run on
Node. Their npm engine range is `>=22 <23 || >=24 <25`; their `kumi` refuses
other majors (except `kumi doctor`, which says what's wrong). Their bridge's
server and `ableton-mcp-setup` refuse them too; `ableton-mcp-diagnostics`
reports them; `ableton-mcp-lifecycle` and `ableton-mcp-migrate` still run, so
an older installation can be inspected or removed.

## MCP protocol

The bridge speaks MCP over stdio in two protocol eras: `2025-11-25`, with the
initialize handshake, and `2026-07-28`, with per-request metadata and
`server/discover`. In the modern era every result is complete, cache hints are
private with zero TTL, and nothing is pushed unasked; it doesn't offer MRTR,
Tasks or HTTP. Tests cover both eras; no particular MCP client or model is
certified. The [user guide](USER_GUIDE.md) covers connecting a client.

## Accessibility

`KUMI_UI=plain` (or piping the output) gives Kumi a plain line-by-line interface
that suits screen readers; see [plain mode](KUMI_TUI.md#plain-mode). The
bridge's own output is plain text in a fixed order, with no colour-only states
and nothing that needs a pointer. Neither has been tested with VoiceOver or
Narrator, and Live, plug-in windows and MCP clients behave as their own
makers decide.

## What CI covers

CI runs on GitHub's hosted runners. After each merge, on Ubuntu: Rust tests,
and the Remote Script's, the Live extension's and Renoise's tool's tests. Each
night and for each release, on macOS, Ubuntu and Windows: six target bundles,
installed with REAPER's extension loaded on each system, and for each release
the migration tests. None has Live, REAPER or Renoise. [Testing](TESTING.md#ci)
lists every job.
