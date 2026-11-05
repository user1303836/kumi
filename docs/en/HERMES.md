# Kumi in Hermes Agent

English · [简体中文](../zh-CN/HERMES.md) · [日本語](../ja/HERMES.md)

Kumi is also a plugin for [Hermes Agent](https://hermes-agent.nousresearch.com). Hermes stays your agent, in its
terminal, Hermes Desktop, the web dashboard or a chat app; Kumi gives it your DAW. Hermes asks before every change,
Kumi records each one in its HISTORY with an undo, and both run on your computer.

Kumi on its own (the `kumi` app in a terminal) and Kumi in Hermes are separate installations. You don't need one to
use the other.

## Install

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

In Hermes Desktop, also switch on Kumi under **Capabilities → Plugins**: a plugin's Desktop half starts switched off.

## Set up

Ask Hermes to "set up Kumi for Live" (or REAPER, or Renoise), or run `hermes kumi setup` in a terminal. Setup has two
steps, and Hermes asks before each:

1. **Kumi's runtime:** Kumi's official release for your computer, 25–32 MB, from Kumi's GitHub releases. It's checked
   against the SHA-256 the plugin pins and unpacked into Hermes's own data for this profile (77–113 MB).
2. **Kumi's part in your DAW:** Live's Remote Script (with the Kumi Ears device, for listening to tracks), REAPER's
   extension or Renoise's tool, at the exact paths setup shows you.

Then, in your DAW:
- **Live:** choose **AbletonMcpBridge** as a Control Surface in Settings → Link, Tempo & MIDI.
- **REAPER:** restart REAPER.
- **Renoise:** enable the Kumi tool.

If Kumi on its own already installed its part in your DAW, setup says so and offers to use that installation instead of
replacing it. Kumi in Hermes then reads that installation's connection files and writes its own connection state there,
never its sign-in or settings.

Setup never starts or quits your DAW, chooses a Control Surface, or saves your project.

## What you can ask

- "What am I looking at?" Kumi reads the track, device and clip you have selected.
- "Find me a darker snare." Kumi searches the sample folders you listed; in Hermes Desktop the sound map opens.
- "Put it on pad 2." Hermes shows exactly what will change and asks; Kumi makes the change and adds it to HISTORY.
- "Is it actually darker?" Kumi measures both files: loudness, true peak, spectrum.
- "Undo the snare change." Kumi undoes that entry while it can still be undone.

## Where things are

- **Every surface:** Kumi's tools, its skills (`kumi:daw`, `kumi:sounds`, `kumi:listening`), `/kumi` for status, and
  `@kumi:focus`, `@kumi:history` or `@kumi:status` in a message to attach what Kumi last saw.
- **Hermes Desktop:** a status chip, a side pane with FOCUS and HISTORY, the sound map in its own tab, cards for
  measured audio, and palette commands. Buttons put a request into the chat; they never change your project by
  themselves.
- **Web dashboard:** a Kumi tab with HISTORY, status and the map.

## Settings

| Setting | Default | What it does |
|---|---|---|
| DAW | Live | Which DAW Kumi connects to |
| Sample folders | none | The folders Kumi may read for your sound map. Nothing else is scanned |
| Tell the agent what you're looking at | on | Adds a short FOCUS and catch-up note to each turn while Kumi is connected |
| Interpret measurements | off | Also sends measurements and a small band-energy image to your Hermes model for a plain-words reading |

## What leaves your computer

- **Setup** downloads Kumi's runtime from Kumi's GitHub releases, once per Kumi version.
- **Your conversation:** the project details Kumi notes (FOCUS, what changed) and its measurements go to the model you
  use in Hermes, as part of the chat.
- **Interpret measurements** (off unless you turn it on) sends the measured numbers and a small image of the measured
  band energy, never the audio or file paths, to the model you choose for Kumi's listening task in Hermes, or to the
  fallback providers Hermes is set to try for that task. Retries and fallbacks can add time and token cost.
- **Audio stays here:** Kumi measures audio on your computer and sends none of it to a model. In a messaging app
  (Telegram, Discord…), a sound you ask Hermes to send you arrives there as an attachment, through that app.
- **Listening to a track or the mix** asks first, and plays only the beat window you approve (60 seconds at most),
  only while the DAW is stopped. Routing and transport may change briefly and are put back after. Live captures it
  through Kumi Ears (Max for Live); REAPER and Renoise render it into the listening folder of the Kumi installation
  that owns their part, and Kumi copies that one file and removes it. A REAPER or Renoise track needs Kumi's newer
  part in that DAW: when Kumi says so, run setup again and restart the DAW. Renoise listens only to a window that
  starts and ends on whole pattern lines inside the song, and refuses any other rather than play more than you
  approved. REAPER may show the project as modified after a track listen, since only saving clears that flag; Kumi
  never saves.
- Kumi in Hermes never uses Kumi's own sign-in, model providers or updater, and sends no telemetry.

## Limits

- Your DAW must run on the computer that runs Hermes. Hermes Desktop connected to a remote Hermes controls that
  computer's DAW, not yours.
- Live works on macOS and Windows; REAPER and Renoise on macOS, Windows and Linux.
- Kumi reads WAV and AIFF itself. For MP3, FLAC and other compressed audio it uses an ffmpeg already on your
  computer, if you allow it at setup; it never downloads one. Before each start Kumi checks that it's still the ffmpeg
  file you allowed (not the libraries it loads, which an upgrade can change). A file it measures can be up to 50 MB.
- Kumi's REAPER extension keeps an id for each saved project in the project (ext state), and its Renoise tool one for
  each song in the song's tool data, saved with them.
- Kumi in Hermes leaves out what would act without Hermes's approval: running code inside the DAW, driving Live's
  menus, recording, and saving or opening projects.

## If something isn't working

- `/kumi` (or `hermes kumi status` in a terminal) says what setup still needs, and whether Kumi can read compressed
  audio. With `plugins.isolation: host`, Hermes leaves plugin commands such as `hermes kumi` out: set Kumi up from a
  chat instead.
- Listening to a REAPER or Renoise track says Kumi's part in the DAW is too old: run setup again, then restart the DAW.
- Nothing from Kumi in Hermes Desktop: check that Kumi is switched on under Capabilities → Plugins, and enabled in
  Hermes (`hermes plugins enable kumi`).
- Live doesn't connect: check that AbletonMcpBridge is chosen as a Control Surface.
- Listening to a Live track says Kumi Ears is missing: run setup again. If Kumi in Hermes uses another Kumi
  installation, run `kumi bridge` in that Kumi instead. Kumi Ears needs Max for Live.
