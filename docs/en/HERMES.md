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

- "What am I looking at?" Kumi reads the track, device and clip you have selected.
- "Find me a darker snare." Kumi searches the sample folders you listed; in Hermes Desktop the sound map opens.
- "Stop the scan." A long scan of your sample folders stops when you ask. You keep the sound map you had, unless the
  scan had already begun saving its new one.
- "Put it on pad 2." Hermes shows exactly what will change and asks; Kumi makes the change and adds it to HISTORY.
- "Is it actually darker?" Kumi measures both files: loudness, true peak, spectrum.
- "Undo the snare change." Kumi undoes that entry while it can still be undone.
- "Make a riser with ElevenLabs and put it on the FX track." When another Hermes plugin has made a sound, Kumi can
  place or measure that finished file. Placing it is a change Hermes asks about; Kumi copies the file into its own data
  first, and the project plays it from there.
- "Make a vinyl crackle with Mirelo" or "Turn this bassline into MIDI." With Mirelo set up (below), Kumi asks Mirelo
  for a sound effect or for MIDI notes, after you approve its estimate.

Hermes asks before each change because its approvals are on. Allowing a Kumi call for the session or always covers
that kind of call (changes, undos, sample-folder adds and scans) in this chat only, including a one-shot run that
resumes it (`hermes chat -q --resume`); another chat, a new one-shot run or a scheduled job asks again or is refused.
An upload to Mirelo is allowed that way for its one file, in this chat. A setup step's approval covers its own plan,
and a paid Mirelo generation's only its own estimate, once. Choosing **Always** leaves an entry in Hermes's
`command_allowlist` (your profile's config.yaml) that Kumi never edits: to remove it, delete the `plugin_rule:kumi:`
entries there.

With `--yolo`, a chat's `/yolo` or `approvals.mode: off`, Hermes runs Kumi's changes, undos, sample-folder adds and
scans, setup steps, and paid Mirelo generations and audio uploads (up to your credit cap) without asking; `/kumi` says
so, as `hermes kumi status` does when `approvals.single_query_mode` or `delegation.subagent_auto_approve` lets changes
pass unasked. A one-shot `hermes -z` turn is yolo too: Kumi's calls run without asking there, and Kumi can't tell it
from a chat.

## Where things are

- **Every surface:** Kumi's tools, its skills (`kumi:daw`, `kumi:sounds`, `kumi:listening`, `kumi:generate`), `/kumi`
  for status, and `@kumi:focus`, `@kumi:history` or `@kumi:status` in a message to attach what Kumi last saw.
- **Hermes Desktop:** a status chip, a side pane with FOCUS and HISTORY, the sound map in its own tab, cards for
  measured audio, and palette commands. Buttons put a request into the chat; they never change your project by
  themselves.
- **Web dashboard:** a Kumi tab with HISTORY, status and the map.

## Mirelo (optional, paid)

With a [Mirelo](https://mirelo.ai) account, Kumi can ask Mirelo for sound effects from a prompt and for MIDI notes from
an audio file. Both are off until you turn them on, and each generation uses Mirelo API credits.

- **Turning it on:** in Kumi's plugin settings, add your Mirelo API key (kept as a Hermes secret), turn on **Use Mirelo
  for paid generation** and **Mirelo sound effects**, **Mirelo audio to MIDI** or both, and set a **Mirelo credit cap**
  above 0. A key alone never makes Kumi use Mirelo.
- **Each generation asks first,** with Mirelo's estimate in API credits (an estimate, never a price). Sending an audio
  file for MIDI is approved on its own, before the estimate. Nothing is paid for or uploaded in a scheduled job, a
  one-shot `hermes chat -q`, a webhook or Hermes's API server.
- **The credit cap** is the most Kumi may reserve, over its whole life. Each generation reserves up to its estimate. A
  charge may stay unknown until Mirelo reports it (after a lost answer, for example), and its reservation keeps
  counting against the cap meanwhile. If a charge comes in above its estimate, new paid generations are paused until
  you let them start again in a terminal.
- **Your credits,** in a terminal: `hermes kumi credits` shows what's spent, reserved and left, and the reservations
  still open. `hermes kumi credits unfreeze` lets paid generations start again, and
  `hermes kumi credits release <id>` releases a reservation whose charge stayed unknown, once you've checked it on
  Mirelo's dashboard; if Mirelo reports a charge for it later, Kumi still counts it and pauses new paid generations.
  Each asks you first, and they're yours alone: Hermes and its model can't run them.
- **What comes back** is kept in Kumi's data. Putting a sound or the notes in your project is a separate change Hermes
  asks about; a sound you put in stays in Kumi's data, and the project plays it from there (see Limits). MIDI notes
  come timed in seconds, and placing them is a notes change.
- **An audio file for MIDI** is measured whole first: WAV within Mirelo's limits (30 minutes, 50 MiB); MP3 and other
  compressed audio need the ffmpeg allowed at setup.
- **Switching a kind off** stops new generations of it; one already running is still fetched while your key is set.
  Removing the key stops Kumi contacting Mirelo.

## Settings

| Setting | Default | What it does |
|---|---|---|
| DAW | Live | Which DAW Kumi connects to. The first DAW you set up becomes it; switch with `hermes kumi setup --daw <daw> --use` (`ableton`, `reaper` or `renoise`) |
| Sample folders | none | The folders Kumi may read for your sound map. Nothing else is scanned. A folder can't be a whole drive or volume, your home folder or a folder above it, or a folder inside or above Hermes's own (`HERMES_HOME`); `~/` works. Links and junctions never lead Kumi outside the folders you list, and on Windows it skips cloud placeholder files (OneDrive's, for example) |
| Tell the agent what you're looking at | on | Adds a short FOCUS and catch-up note to the chat connected to Kumi when what you're looking at changes |
| Interpret measurements with a model | off | Also sends measurements and a small band-energy image for a plain-words reading to the model set for Kumi's listening task in Hermes (Vision's, unless you change it), which must accept images |
| Mirelo API key | none | Your Mirelo key, kept as a Hermes secret (`MIRELO_API_KEY`). Kumi sends it only to Mirelo, with a generation you approved |
| Use Mirelo for paid generation | off | Lets Hermes ask for Mirelo generations, each approved with its estimate |
| Mirelo sound effects / Mirelo audio to MIDI | off | Each kind is allowed only when it's on |
| Mirelo credit cap | 0 | The most Mirelo API credits Kumi may reserve, over its whole life. 0 refuses every paid generation |

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
- **Mirelo** (only if you turn it on): your prompt, and for MIDI the audio file you approve, go to Mirelo with your
  key: to `api.mirelo.ai`, and to the upload and download links Mirelo's API gives for each job, on hosts Mirelo
  chooses (its storage or delivery network), so they aren't a fixed list. Whether Mirelo may train on what you send
  follows your Mirelo account's settings; Kumi has no setting of its own for it. What Mirelo makes comes back to
  Kumi's data.
- **Scheduled jobs** (Hermes cron) read only Kumi's status and HISTORY as last published. Their answer (change titles,
  track names) leaves your computer only through where the job delivers; Kumi's job blueprints deliver locally by
  default.
- Kumi for Hermes never uses a standalone Kumi's sign-in, model providers or updater, and sends no telemetry.

## Limits

- Your DAW must run on the computer that runs Hermes. Hermes Desktop connected to a remote Hermes controls that
  computer's DAW, not yours.
- Live works on macOS and Windows; REAPER and Renoise on macOS, Windows and Linux.
- Kumi for Hermes doesn't listen to a track or the mix yet, in any DAW: it refuses and plays nothing. It measures audio
  files, library sounds and Kumi's own results.
- A sound Kumi made or copied (Mirelo's, or another Hermes plugin's such as ElevenLabs's) and you put in a project
  stays in Kumi's data, this profile's `plugin-data/kumi/` folder, and the project plays it from there. Before you
  delete that folder, save such projects with their files copied in (Live: **File → Collect All and Save**; REAPER:
  **Save project as…** with **Copy all media into project directory**), or they'll miss those samples.
- Kumi reads only the open project, the sample folders you list, and finished audio other Hermes plugins saved in
  Hermes's generated-audio folder (`<HERMES_HOME>/cache/generated/audio`, which Kumi never creates), to place or
  measure it.
- Kumi reads WAV and AIFF itself. For MP3, FLAC and other compressed audio it uses an ffmpeg already on your
  computer, if you allow it at setup; it never downloads one. Before each start Kumi checks that it's still the ffmpeg
  file you allowed (not the libraries it loads, which an upgrade can change). ffmpeg gets only the one file Kumi is
  reading: never a URL or a playlist, and nothing from the network. A file Kumi reads can be up to 50 MiB, and it
  measures at most 12 minutes of it.
- Kumi's REAPER extension keeps an id for each saved project in the project (ext state), and its Renoise tool one for
  each song in the song's tool data, saved with them.
- On Windows, a sample in a network folder (`\\server\share`, or a share reached through a link) can't be played in
  Hermes Desktop or the dashboard: they play only files on this computer's own drives.
- Stop in Hermes Desktop or Hermes's terminal UI, or `/stop` or `/new` in a chat app, cancels a Kumi change or undo
  still running. Kumi can't then tell whether it finished, so check HISTORY before asking again. In the classic
  `hermes chat` CLI, Stop doesn't reach Kumi: the call runs until it finishes or reaches its time limit.
- Kumi can undo a change only while the Kumi runtime that made it is still running. The runtime stops when its chat
  ends, when Hermes unloads the plugin or exits, or after 15 minutes with nothing to do (2 hours while it's connected
  to your DAW for a chat). A job it's running, such as a scan or a Mirelo generation, keeps it going for up to 6 hours.
  After it stops, the change stays in HISTORY, and your DAW's own Undo is the way back.
- In Live, Kumi won't undo a change to a track's mixer, the tempo or a device's parameters if you've changed that
  value by hand since: the undo says so and changes nothing. Before it asks, an undo shows each value as it is now and
  as the undo would put it back. In REAPER and Renoise, an undo puts the value back even if you've changed it by hand
  since.
- Kumi for Hermes leaves out what would act without Hermes's approval: running code inside the DAW, driving Live's
  menus, recording, and saving or opening projects.

## If something isn't working

- `hermes kumi doctor` (or `/kumi doctor` in a chat) checks what Kumi needs in this Hermes profile: Hermes's approval
  settings, setup, the connection, the open project and the sound map. For each problem it says what to do. It changes
  nothing.
- `/kumi` (or `hermes kumi status` in a terminal) says what setup still needs, and whether Kumi can read compressed
  audio. With `plugins.isolation: host`, Hermes leaves plugin commands such as `hermes kumi` out: set Kumi up from a
  chat in Hermes Desktop instead.
- A paid generation refused for credits: `hermes kumi credits` shows what's spent, reserved and left, and whether new
  paid generations are paused because a charge came in above its estimate.
- Nothing from Kumi in Hermes Desktop: check Kumi's **Agent** and **Desktop** switches under Capabilities → Plugins,
  and that it's enabled in Hermes (`hermes plugins enable kumi`).
- No Kumi tab in the web dashboard: if the dashboard was already running when you installed Kumi, restart it
  (`hermes dashboard`).
- Live doesn't connect: check that AbletonMcpBridge is chosen as a Control Surface.
