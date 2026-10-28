<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="REAPER 7" src="https://img.shields.io/badge/REAPER-7-111111">
  <img alt="Renoise 3.5" src="https://img.shields.io/badge/Renoise-3.5-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a> · <a href="README.ja.md">日本語</a>
</p>

**Kumi is a general-purpose AI agent that works alongside you in your DAW.** Kumi currently supports Ableton Live, REAPER and Renoise, with the same abilities in each: it reads your project, changes almost anything in it, listens to the result and learns your sounds, your taste and your habits. You ask in plain words, in a chat that runs in your terminal. Kumi plans the change, makes it and lists each step in its history with an undo. The longer you use Kumi, the more familiar Kumi is with how you work, your preferences, and your patterns as a producer.

https://github.com/user-attachments/assets/f53fe0be-c9a9-476e-a1d5-efe3dc429526

## Quickstart

Get started with one command in Terminal on a Mac or PowerShell on Windows.

On a Mac or Linux:

```sh
curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh
```

On Windows:

```powershell
irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex
```

Follow Kumi's guided setup.

Next time, open a terminal and type `kumi`, then open your DAW (Live, REAPER or Renoise, before or after Kumi): Kumi connects to the one that's open.

> [!NOTE]
> Kumi runs in a terminal window, but you don't need to know any commands. After the install line, it's a chat. Drag across text to select it, then press Ctrl-C to copy.

## What can I ask it?

- **Build a sound from a tutorial.** Paste a YouTube link: Kumi watches it and builds what it shows on a new track.
- **Fix the mix.** Ask “is the bass muddy?” and Kumi listens, finds what's clashing and fixes it, listening again after each change.
- **Match a reference.** Give it a track you love: Kumi tries a few takes, scores each against it and refines the best.
- **Make a device.** Describe a Max for Live effect or instrument and Kumi builds it and puts it on your track.
- **Find your sounds.** “A dusty snare like this one”, “my usual vocal chain”: Kumi knows your samples, presets and projects.
- **Map your sounds.** `/map` lays out your samples and songs so similar ones sit together; point at one to hear it. Gather the ones you like into a palette, and Kumi picks from it when you ask for “a darker kick from my palette”, or loads the whole palette as a kit.
- **Do the chores.** Renaming, colouring, routing, sidechains, grouping, freezing, bouncing and exporting.

It works the same way in all three DAWs, each in its own words: [what works where](docs/en/DAWS.md).

## Questions

<details>
<summary><b>Do I need to know how to code?</b></summary>

No. You work with Kumi in plain words, typed or spoken. When a job needs code (Python in Live, Lua in REAPER and Renoise), Kumi writes and runs it itself.

</details>

<details>
<summary><b>Will it mess up my project?</b></summary>

Every change is listed in Kumi's history (“Tempo 120 → 124 BPM”), most with their own undo. In Live, each request is also one step in Live's own undo, and Kumi copies the last saved version of your Set before a large or destructive change.

</details>

<details>
<summary><b>Which DAWs and computers?</b></summary>

- Ableton Live 12 on macOS 13+ or Windows 10/11
- REAPER 7+ on macOS, Windows or Linux
- Renoise 3.5+ on macOS, Windows or Linux

With more than one installed, setup asks which you work in; `/daw` switches later.

</details>

<details>
<summary><b>Which AI does it use, and what does it cost?</b></summary>

Kumi is free and open source (MIT). You choose the model:

- **ChatGPT plan:** sign in with your account; Kumi works within your plan's usage limits.
- **API key:** Anthropic, OpenAI, Google Gemini, OpenRouter and [20 more](docs/en/KUMI_GUIDE.md#sign-in-and-choose-a-model), billed by the provider.
- **Local:** Ollama, LM Studio or any OpenAI-compatible server such as llama.cpp or vLLM, on hardware that can run a tool-calling model with a long context (Kumi asks for about 57k tokens).

</details>

<details>
<summary><b>Does my audio leave my computer?</b></summary>

Audio is analysed locally; the model gets the measurements. Your library and music collection are indexed locally too. With a Gemini or OpenAI API key, a listening model also hears mono excerpts (10 seconds at most) of the changes it judges; `/slots listening off` turns it off. [Privacy](docs/en/KUMI_GUIDE.md#privacy-what-leaves-your-computer) covers the rest.

</details>

<details>
<summary><b>Something isn't working</b></summary>

`kumi doctor` checks the install, sign-in, bridge and DAW connection, and says what to fix. `kumi report` writes a file to attach to an [issue](https://github.com/user1303836/kumi/issues).

</details>

[Guide](docs/en/KUMI_GUIDE.md) · [Commands, keys and screens](docs/en/KUMI_TUI.md) · [How Kumi changes your project](docs/en/KUMI_CHANGES.md) · [Changelog](CHANGELOG.md)

## Status

- Kumi 1.24.9 is currently supported for Ableton Live 12.4.15b4 (beta) and above on macOS and Windows. You may run into issues on lower versions. If you do, **please file an issue!**
- 1.24.9 is tested with Live on macOS. On Windows, installing and updating are tested; using it with Live there is still new. Please send a `kumi report` when something breaks.
- REAPER 7.82 and Renoise 3.5.4 are tested on macOS. On Windows and Linux, CI tests putting Kumi's extension and tool in place and loading the extension; using them with REAPER and Renoise there is new.

## Development

To build Kumi from source, see [DEVELOPMENT.md](DEVELOPMENT.md) and the [developer guide](docs/en/DEVELOPER_GUIDE.md).

## License

[MIT](LICENSE.md); the parts from other people's work, and the models Kumi fetches, are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Ableton Live is a trademark of Ableton AG; Kumi is not affiliated with or endorsed by Ableton, Cockos (REAPER) or Renoise.
