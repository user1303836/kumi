# Kumi for Hermes Agent

English · [简体中文](../zh-CN/HERMES.md) · [日本語](../ja/HERMES.md)

Kumi is also a plugin for [Hermes Agent](https://hermes-agent.nousresearch.com). Hermes stays your agent, in its
terminal, Hermes Desktop, the web dashboard or a chat app; Kumi gives it your DAW. Hermes asks before every change
(unless you've turned its approvals off), Kumi records each one in its HISTORY, with an undo while Kumi runs, and both
run on your computer.

A standalone Kumi (the `kumi` app in a terminal) and Kumi for Hermes are separate installations. You don't need one to
use the other.

## Install

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

`kumi` is Kumi's name in Hermes's plugin catalog. Until Kumi is listed there, install it from Kumi's repository with
`hermes plugins install user1303836/kumi#hermes-plugin`, then enable it the same way.

In Hermes Desktop, also turn on Kumi's **Desktop** switch under **Capabilities → Plugins**: a plugin's Desktop switch
starts off.

## Set up

Ask Hermes to "set up Kumi for Live" (or REAPER, or Renoise), or run `hermes kumi setup --daw reaper` in a terminal
(`--daw` takes `ableton`, `reaper` or `renoise`; without it, setup is for the DAW in Kumi's settings, Live unless you've
changed it). Setup has two steps and an optional third, and Hermes asks before each:

1. **Kumi's runtime:** Kumi's official release for your computer, 26–33 MB, from Kumi's GitHub releases. It's checked
   against the SHA-256 the plugin pins and unpacked into Hermes's own data for this profile (82–121 MB).
2. **Kumi's part in your DAW:** Live's Remote Script (with the Kumi Ears device), REAPER's
   extension or Renoise's tool, at the exact paths setup shows you. Quit Live first: its Remote Script can't be
   installed while Live runs. REAPER and Renoise can stay open; restart them after (on Windows, quit REAPER to update
   its extension).
3. **Optional, ffmpeg:** if an ffmpeg is already on your computer, setup offers to let Kumi use it to read MP3, FLAC
   and other compressed audio. Setup is done either way; without it, Kumi reads WAV and AIFF only.

Hermes Desktop shows each step's approval whole. A terminal chat or a chat app can show less of it, so there Kumi never
asks with part of a step: it asks you to run `hermes kumi setup` in a terminal, or to use Hermes Desktop.

Then, in your DAW:
- **Live:** choose **AbletonMcpBridge** as a Control Surface in Settings → Link, Tempo & MIDI.
- **REAPER:** restart REAPER.
- **Renoise:** enable the Kumi tool.

The first DAW you set up becomes Kumi's DAW (the DAW setting), in the same approval. Setting up another DAW later
leaves Kumi with the one it has; setup then names the step that switches: `hermes kumi setup --daw <daw> --use` in a
terminal, which asks before it changes the setting. You can also change the DAW in Kumi's plugin settings
(`plugins.entries.kumi.settings.daw` in your profile's config.yaml). Where Hermes's settings can't be written, setup
says so and where to set the DAW instead.

If another Kumi on this computer (a standalone Kumi, or Kumi for Hermes in another Hermes profile) already put its part
in Live, setup says so and offers to use that installation instead of replacing it. Kumi for Hermes then reads that
installation's connection files and writes its own connection state there, never its sign-in or settings. For REAPER
and Renoise, Kumi for Hermes can't use another Kumi's part yet: setup says so and leaves it as it is.

Setup never starts or quits your DAW, chooses a Control Surface, or saves your project.

## What you can ask

### Master your mix like a reference track

"Listen to my mix and master it like this reference track."
- **Listen:** Kumi listens to your mix in Live, REAPER or Renoise; each listen asks first.
- **Compare:** it measures your mix beside the reference file: loudness, true peak, spectral balance, stereo width and
  dynamics.
- **Master:** it proposes mastering moves on the master track (EQ, glue compression, a limiter) as one change Hermes
  asks about.
- **Repeat:** it listens again and compares, for at most three rounds. Each round's change is in HISTORY, and you can
  undo it.

### Write a beat or a bassline

"Write a four-on-the-floor kick with offbeat hats on a new track", then "now a bassline in A minor under it."
- Kumi writes the notes as one change Hermes asks about.
- "Play it" plays the project so you can hear it, and asks first; "Stop" stops it and never asks. Neither changes
  anything in the project but where play starts. In Live, "play it" launches a Session clip Kumi just wrote.

### Find a sample and put it on a pad

"Find me a dusty 90 BPM snare in A minor."
- Kumi searches the sample folders you listed by sound, tempo and key, and says why each result matches. In Hermes
  Desktop the sound map opens.
- "Put it on pad 2" places it, on an empty pad too, as one change Hermes asks about. In REAPER it goes on a track; in
  Renoise, into an instrument.

### And the everyday ones

- "What am I looking at?" Kumi reads the track, device and clip you have selected.
- "Make the bass 3 dB quieter." Hermes shows exactly what will change and asks; Kumi makes the change and adds it to
  HISTORY.
- "Compare these two bounces." Kumi measures both files: loudness, true peak, spectrum.
- "Stop the scan." A long scan of your sample folders stops when you ask. You keep the sound map you had, unless the
  scan had already begun saving its new one.
- "Undo the snare change." Kumi undoes that entry while it can still be undone.

Kumi also works with sounds from ElevenLabs and Mirelo: see [Integrations](#integrations-optional).

Hermes asks before each change, listen and play because its approvals are on ("Stop" never asks). Allowing a Kumi
call for the session or always covers that kind of call (changes, undos, listens, play, sample-folder adds and scans)
in this chat only, including a one-shot run that resumes it (`hermes chat -q --resume`); another chat, a new one-shot
run or a scheduled job asks again or is refused. A setup step's approval covers its own plan. Choosing **Always**
leaves an entry in Hermes's `command_allowlist` (your profile's config.yaml) that Kumi never edits: to remove it,
delete the `plugin_rule:kumi:` entries there.

With `--yolo`, a chat's `/yolo` or `approvals.mode: off`, Hermes runs Kumi's changes, undos, listens, play,
sample-folder adds and scans, setup steps, and, with Mirelo set up, its paid generations and uploads (up to your
credit cap) without asking; `/kumi` says so, as `hermes kumi status` does when `approvals.single_query_mode` or
`delegation.subagent_auto_approve` lets changes pass unasked. A one-shot `hermes -z` turn is yolo too: Kumi's calls run
without asking there, and Kumi can't tell it from a chat.

## Where things are

- **Every surface:** Kumi's tools, its skills (`kumi:daw`, `kumi:sounds`, `kumi:listening`, and `kumi:generate` for
  Mirelo), `/kumi` for status, and `@kumi:focus`, `@kumi:history` or `@kumi:status` in a message to attach what Kumi
  last saw.
- **Hermes Desktop:** a status chip, a side pane with FOCUS and HISTORY, the sound map in its own tab, cards for
  measured audio, and palette commands. Buttons put a request into the chat; they never change your project by
  themselves.
- **Web dashboard:** a Kumi tab with HISTORY, status and the map.

## Settings

| Setting | Default | What it does |
|---|---|---|
| DAW | Live | Which DAW Kumi connects to. The first DAW you set up becomes it; switch with `hermes kumi setup --daw <daw> --use` (`ableton`, `reaper` or `renoise`) |
| Sample folders | none | The folders Kumi may read for your sound map: a scan reads only the audio files in them, and nothing else is scanned. A folder can't be a whole drive or volume, your home folder or a folder above it, or a folder inside or above Hermes's own (`HERMES_HOME`); `~/` works. Links and junctions never lead Kumi outside the folders you list, and on Windows it skips cloud placeholder files (OneDrive's, for example) |
| Tell the agent what you're looking at | on | When what you're looking at changes, adds a short note to the chat connected to Kumi: what you're on (FOCUS), and what changed in the project while Kumi was away |
| Interpret measurements with a model | off | Also sends measurements and a small band-energy image for a plain-words reading to the model set for Kumi's listening task in Hermes (Vision's, unless you change it), which must accept images |

## What leaves your computer

- **Setup** downloads Kumi's runtime from Kumi's GitHub releases, once per Kumi version.
- **Your conversation:** the project details Kumi notes (FOCUS, what changed) and its measurements go to the model you
  use in Hermes, as part of the chat. Kumi's status tells it where setup stands without naming any folder on your
  computer.
- **Interpret measurements with a model** (off unless you turn it on) sends the measured numbers and a small image of
  the measured band energy, never the audio or file paths, to the model set for Kumi's listening task in Hermes, or to
  the fallback providers Hermes is set to try for that task. Retries and fallbacks can add time and token cost.
- **Audio stays here:** Kumi measures audio on your computer and sends none of it to a model. In a messaging app
  (Telegram, Discord…), a sound you ask Hermes to send you arrives there as an attachment, through that app.
- **Listening to a track or the mix:**
  - **Approval and window:** it asks first and plays only the window you approve, 60 seconds at most, starting with
    the DAW stopped.
  - **Your project:** routing and transport may change briefly, and are put back after.
  - **How each DAW hears it:** in Live, Kumi's Ears device captures the window as Live plays it. REAPER and Renoise
    render it into the listening folder of the Kumi installation that owns their part; Kumi copies that one file and
    removes it.
  - **Renoise's window:** it listens only to a window that starts and ends on whole pattern lines inside the song, and
    refuses any other rather than play more than you approved.
  - **Renoise's tempo:** rendering can leave its BPM and lines per beat at the values the song's own tempo commands
    reach by the window's end. When the listen ends, fails or is cancelled, Kumi sets both back if they still have
    exactly those values. If either is different, Kumi leaves both as they are and says it couldn't restore the
    tempo.
  - **REAPER's "modified" flag:** REAPER may show the project as modified after a listen, since only saving clears that
    flag; Kumi never saves.
- **Mirelo,** only if you set it up: see [Integrations](#integrations-optional).
- **Scheduled jobs** (Hermes cron) read only Kumi's status and HISTORY as last published. Their answer (change titles,
  track names) leaves your computer only through where the job delivers; Kumi's job blueprints deliver locally by
  default.
- Kumi for Hermes never uses a standalone Kumi's sign-in, model providers or updater, and sends no telemetry.

## Limits

- Your DAW must run on the computer that runs Hermes. Hermes Desktop connected to a remote Hermes controls that
  computer's DAW, not yours.
- Live works on macOS and Windows; REAPER and Renoise on macOS, Windows and Linux.
- Kumi listens to a track or the mix only in the DAWs your Kumi release can. Elsewhere it says so and plays nothing,
  and it still measures audio files, library sounds and Kumi's own results.
- Kumi reads only the open project, the audio files in the sample folders you list, and finished audio other Hermes
  plugins saved in Hermes's generated-audio folder (`<HERMES_HOME>/cache/generated/audio`, which Kumi never creates),
  to place or measure it.
- Kumi reads WAV and AIFF itself. For MP3, FLAC and other compressed audio it uses an ffmpeg already on your
  computer, if you allow it at setup; it never downloads one. Before each start Kumi checks that it's still the ffmpeg
  file you allowed (not the libraries it loads, which an upgrade can change). ffmpeg gets only the one file Kumi is
  reading: never a URL or a playlist, and nothing from the network. A file Kumi reads can be up to 50 MiB, and it
  measures at most 12 minutes of it.
- Kumi's REAPER extension keeps an id for each saved project in the project (ext state), and its Renoise tool one for
  each song in the song's tool data, saved with them.
- On Windows, a sample in a network folder (`\\server\share`, or a share reached through a link) can't be played in
  Hermes Desktop or the dashboard: they play only files on this computer's own drives.
- Stop in Hermes Desktop or Hermes's terminal UI, or `/stop` or `/new` in a chat app, cancels a Kumi change, undo or
  listen still running. Kumi can't then tell whether it finished, so check HISTORY before asking again. In the classic
  `hermes chat` CLI, Stop doesn't reach Kumi: the call runs until it finishes or reaches its time limit.
- Kumi can undo a change only while the Kumi runtime that made it is still running. The runtime stops when its chat
  ends, when Hermes unloads the plugin or exits, or after 15 minutes with nothing to do (2 hours while it's connected
  to your DAW for a chat). A job it's running, such as a library scan, keeps it going for up to 6 hours.
  After it stops, the change stays in HISTORY, and your DAW's own Undo is the way back.
- In Live, Kumi won't undo a change to a track's mixer, the tempo or a device's parameters if you've changed that
  value by hand since: the undo says so and changes nothing. Before it asks, an undo shows each value as it is now and
  as the undo would put it back. In REAPER and Renoise, an undo puts the value back even if you've changed it by hand
  since.
- Kumi for Hermes leaves out what would act without Hermes's approval: running code inside the DAW, driving Live's
  menus, recording, and saving or opening projects.

## Integrations (optional)

Core Kumi needs neither of these.

### Sounds from other Hermes plugins (ElevenLabs)

"Make a riser with ElevenLabs and put it on the FX track." When another Hermes plugin has made a sound, Kumi can
measure it or put it in your project. It reads only finished audio in Hermes's generated-audio folder
(`<HERMES_HOME>/cache/generated/audio`, which Kumi never creates) and copies the file into its own data first; placing
it is a change Hermes asks about.

### Mirelo (paid)

"Make a vinyl crackle with Mirelo" or "Turn this bassline into MIDI." With a [Mirelo](https://mirelo.ai) account, Kumi
can ask Mirelo for a sound effect from a prompt or for MIDI notes from an audio file.

- **Off until you turn it on:** in Kumi's settings, add your **Mirelo API key** (a Hermes secret, `MIRELO_API_KEY`),
  turn on **Use Mirelo for paid generation** and the kinds you want (**Mirelo sound effects**, **Mirelo audio to
  MIDI**), and set a **Mirelo credit cap** above 0 (the default, 0, refuses every paid generation). A key alone never
  makes Kumi use Mirelo.
- **Credits, never money:** each generation asks first with Mirelo's estimate in API credits, and that approval
  covers only that estimate, once. Sending an audio file for MIDI is approved on its own; allowing it for the session
  covers only that file, in that chat. The cap counts over Kumi's whole life: each generation reserves up to its
  estimate, a charge may stay unknown until Mirelo reports it (its reservation keeps counting meanwhile), and a charge
  above its estimate pauses new paid generations. Nothing is paid for or uploaded in scheduled jobs,
  `hermes chat -q`, webhooks or Hermes's API server.
- **Your credits, in a terminal:** `hermes kumi credits` shows what's spent, reserved and left, and the open
  reservations. `hermes kumi credits unfreeze` lets paid generations start again; `hermes kumi credits release <id>`
  releases a reservation whose charge stayed unknown, after you've checked Mirelo's dashboard (a charge Mirelo reports
  for it later still counts, and pauses new paid generations). Each asks you first, and they're yours alone: Hermes
  and its model can't run them.
- **What leaves:** your prompt, and for MIDI the audio file you approve, go with your key to `api.mirelo.ai` and to
  the upload and download links Mirelo's API gives for each job, on hosts Mirelo chooses (its storage or delivery
  network), so they aren't a fixed list. Whether Mirelo trains on what you send follows your Mirelo account's
  settings; Kumi has no setting of its own for it.
- **What comes back** is kept in Kumi's data. Placing it is a separate change Hermes asks about, and MIDI notes come
  timed in seconds. An audio file for MIDI must fit Mirelo's limits (30 minutes, 50 MiB); MP3 and other compressed
  audio need the ffmpeg allowed at setup. Switching a kind off stops new generations of it (one already running is
  still fetched while your key is set); removing the key stops Kumi contacting Mirelo.

### Their sounds in your projects

A sound from either that you put in a project stays in this profile's `plugin-data/kumi/` folder, and the project
plays it from there. Before you delete that folder, save such projects with their files copied in (Live: **File →
Collect All and Save**; REAPER: **Save project as…** with **Copy all media into project directory**), or they'll miss
those samples.

## If something isn't working

- `hermes kumi doctor` (or `/kumi doctor` in a chat) checks what Kumi needs in this Hermes profile: Hermes's approval
  settings, setup, the connection, the open project and the sound map. For each problem it says what to do. It changes
  nothing.
- `/kumi` (or `hermes kumi status` in a terminal) says what setup still needs, and whether Kumi can read compressed
  audio. With `plugins.isolation: host`, Hermes leaves plugin commands such as `hermes kumi` out: set Kumi up from a
  chat in Hermes Desktop instead.
- Listening to a track says Kumi's part in the DAW is too old: run setup again, then restart the DAW.
- Nothing from Kumi in Hermes Desktop: check Kumi's **Agent** and **Desktop** switches under Capabilities → Plugins,
  and that it's enabled in Hermes (`hermes plugins enable kumi`).
- No Kumi tab in the web dashboard: if the dashboard was already running when you installed Kumi, restart it
  (`hermes dashboard`).
- Live doesn't connect: check that AbletonMcpBridge is chosen as a Control Surface.
