# Kumi for Hermes

![Kumi for Hermes](../docs/assets/hermes/banner.png)

**Drive Ableton Live, REAPER and Renoise from Hermes.** Kumi reads the project you have open, makes typed changes
in it through Hermes's approval, records every change in its HISTORY, measures audio on your computer, and
maps the sample folders you choose so you can find, audition and place sounds. Hermes stays the agent: Kumi gives it
a DAW to work in.

- **Master your mix like a reference track.** "Listen to my mix and master it like this reference track."
  - Kumi listens to the window of your mix you approve (in Live, through the Kumi Ears device, before Main's fader)
    and measures it beside the reference: loudness, true peak, spectral balance, stereo width and dynamics.
  - It proposes EQ, glue compression and a limiter on the master track as one approved change.
  - It listens again and compares again, for at most three rounds.
- **Write a beat or a bassline.** "Write a four-on-the-floor kick with offbeat hats on a new track", "now a bassline in
  A minor". "Play it" starts playback after you approve; "make three variations" writes takes beside it, never over it.
- **Find a sample and use it.** "Find me a dusty, dark snare", then "use it on the snare" or "put it on pad 2". Kumi
  searches your own sample folders by how each sound measures and by the words in its name and folder, then puts it in
  a Simpler (the snare's, or a new one on a new track) or on a Drum Rack pad, an empty one included (Live), on a new
  track (REAPER) or in a new instrument (Renoise).
- **Build from a tutorial.** "Make the bass from this tutorial: <a YouTube link>." Hermes reads its transcript and,
  for values only on screen, views those moments in its browser; Kumi builds it. No video is downloaded.
- **When no change fits, a script or a device.** Hermes can run a script you approve (Python in Live, Lua in REAPER
  and Renoise), or build a Max for Live audio effect from GenExpr code; each shows its code before you decide.
- **Ask in plain words, from any Hermes surface.** The TUI, Hermes Desktop, the web dashboard and messaging all use the
  same tools.
- **Every change is approved by Hermes (unless its approvals are off), and most are undoable while Kumi runs.** Hermes
  asks before Kumi changes your project and shows exactly what will change; Kumi records the result in its HISTORY,
  with an undo while the Kumi runtime that made the change is running. A few only your DAW's own Undo takes back (in
  Live, a rack chain, a device move or a pad load; a script in any DAW), and Kumi says so before you approve.
- **Kumi knows what you're looking at.** When what you've selected changes, the chat connected to Kumi gets a short
  note of the track, device and clip (FOCUS), and of what changed in the project while Kumi was away.
- **Your own sounds, on a map.** Kumi learns the sample folders you list and lays them out by how they sound; in
  Hermes Desktop the map opens in its own tab, where a click auditions a sound and "Use" asks Hermes to place it.
- **Measured, not imagined.** Loudness, true peak, loudness range, spectrum and tempo are measured on your
  computer. Kumi never claims to have heard what it hasn't measured.

## Tools

| Tool | What it does | Asks first |
|---|---|---|
| `kumi_status` | Setup, runtime and DAW connection state | no |
| `kumi_setup` | Plans and installs Kumi's runtime and its part in your DAW | yes, each step |
| `kumi_connect` | Starts Kumi and connects to the DAW chosen in settings | no (changes nothing in your project) |
| `kumi_observe` | A short, current look at the project: FOCUS, what changed, the project overview | no |
| `kumi_read` | Bounded reads of tracks, devices, clips and their settings | no |
| `kumi_plugin_knowledge` | Kumi's guide to a Live plug-in on a track and the parameters configured for it, or (in any DAW, even with none connected) its catalog of plug-in formats and the controls it has verified | no |
| `kumi_change` | One typed change of one or more steps, recorded in HISTORY | yes |
| `kumi_undo` | Undoes a HISTORY entry while it can still be undone, or your newest change whole ("undo that") | yes |
| `kumi_history` | Lists recent changes and whether each can be undone | no |
| `kumi_listen` | Measures an audio file, a library sound, a result Kumi made, or a window of a track or the mix (in Live, through Kumi Ears, before the fader), optionally beside a reference: part of one (up to 60 s), or up to five measured as one target | a track or the mix: yes |
| `kumi_transport` | Starts playback, or stops it; changes nothing in the project but where play starts | play: yes; stop: no |
| `kumi_variations` | Numbered takes of a clip, item or pattern, each written as a new one, never over it | yes |
| `kumi_script` | Runs a script in the DAW for what no typed change does: Python in Live, Lua in REAPER and Renoise | yes, with its code |
| `kumi_device` | Builds a Max for Live audio effect from GenExpr code, as a new file in your User Library | yes, with its code |
| `kumi_sounds` | Searches your sound library by how sounds measure, by name, and by tempo, key, length and kind, saying why each result matches and on what | no |
| `kumi_library` | Adds a sample folder and scans it | yes |
| `kumi_job` | Reports a long job (a library scan, a Mirelo generation) | no |
| `kumi_generate` | Optional Mirelo integration: sound effects from a prompt and MIDI notes from an audio file (see Integrations) | yes: each paid generation, and each file sent |

Skills teach Hermes the workflows: `kumi:daw`, `kumi:sounds` and `kumi:listening`, plus `kumi:generate` for Mirelo. In
any message, write `@kumi:focus`, `@kumi:history` or `@kumi:status` to attach what Kumi last saw: what you're on in the
DAW, its recent changes, or its connection. They only read what Kumi has already published, so nothing asks first.

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

Kumi needs two things on the machine that runs Hermes, can use a third, and asks before each:

1. **Its runtime:** Kumi's official release for your platform, 26–33 MB, from
   `github.com/user1303836/kumi/releases/download/v<version>/`. Kumi checks it against the SHA-256 this plugin pins,
   unpacks it (82–121 MB) into this profile's plugin data, and refuses anything that doesn't match.
2. **Its part in your DAW:** Live's Remote Script (with the Kumi Ears device), REAPER's extension or Renoise's tool,
   installed only at the location shown to you. Quit Live first; REAPER and Renoise can stay open and restart after.
   If another Kumi on this computer (a standalone Kumi, or Kumi for Hermes in another Hermes profile) already put its
   part in Live, setup offers to use that installation instead of replacing it; for REAPER and Renoise, Kumi for Hermes
   can't use another Kumi's part yet. The first DAW you set up becomes Kumi's `daw` in the same approval; setting up
   another one keeps the current DAW and ends with the step that switches: `hermes kumi setup --daw <daw> --use`,
   approved like the rest.
3. **Optional, ffmpeg:** if an ffmpeg is already on this computer, setup offers to let Kumi's runtime run it to read
   MP3, FLAC and other compressed audio. Setup is done either way; without it, Kumi reads WAV and AIFF only.

Ask Hermes to "set up Kumi for Live" from any surface, or run `hermes kumi setup --daw reaper` in a terminal (`--daw`
takes `ableton`, `reaper` or `renoise`; without it, setup is for the `daw` setting, Live unless you've changed it).
Then, in the DAW:

- **Live:** choose **AbletonMcpBridge** as a Control Surface in Settings → Link, Tempo & MIDI.
- **REAPER:** restart REAPER.
- **Renoise:** enable the Kumi tool.

Setup never starts or quits your DAW, picks a Control Surface for you, or saves a project.

## Commands

| Command | What it does |
|---|---|
| `/kumi` in a chat, `hermes kumi status` in a terminal | Where Kumi stands in this profile: what setup still needs, the DAW connection, whether Kumi can read compressed audio, and whether Hermes's approvals are off |
| `/kumi doctor`, `hermes kumi doctor` | Checks what Kumi needs in this profile and says what to do about each problem. It changes nothing |
| `hermes kumi setup --daw <daw>` | Sets Kumi up for `ableton`, `reaper` or `renoise`, asking before each step; `--use` also makes that DAW Kumi's |

## Settings

Set them in `plugins.entries.kumi.settings` in this profile's `config.yaml`, or where Hermes Desktop shows the plugin's
settings:

| Setting | Default | What it does |
|---|---|---|
| `daw` | Live (`ableton`) | Which DAW Kumi connects to: `ableton`, `reaper` or `renoise`. The first DAW you set up becomes it; `hermes kumi setup --daw <daw> --use` switches |
| `library_folders` | none | The folders Kumi may read to build your sound map: a scan reads only the audio files in them, and nothing else is scanned |
| `inject_focus` | on | When what you're looking at changes, adds a short note to the chat connected to Kumi: what you're on (FOCUS), and what changed in the project while Kumi was away |
| `scripts_in_messaging` | off | Lets a messaging chat (Discord, Slack, Telegram…) ask to run a script in your DAW, or to build a Max for Live device. Each one is asked, with its code, and none runs while Hermes's approvals are off |
| `listen_interpretation` | off | Also asks the model set for Kumi's listening task in Hermes (Vision's, unless you change it; it must accept images) to interpret measurements (see below) |

## Hermes Desktop and the dashboard

- **Desktop:** a status chip (DAW, project, tempo), a side pane with FOCUS and HISTORY, the sound map in its own tab,
  cards for measured audio, and palette commands. While a script or device waits for your approval, the pane shows its
  whole code (and a device's controls); a script's HISTORY row has **Show code**. Buttons never change your project
  directly: they write a request into the chat, which goes through the same approval.
- **Web dashboard:** a Kumi tab with the same HISTORY, status and map.

## Files

Kumi keeps what it writes for Hermes in this profile's `plugin-data/kumi/` folder: the runtime, the library index and
map, HISTORY, measurements, copies of the audio it measured or made (`state/artifacts`; your originals are never
changed), the code of each script you approved (`state/scripts`) and job records. Outside it, setup writes only Kumi's
part in your DAW, at the location you approved, and each Max for Live device you approve is a new file in Live's
User Library (`User Library/Kumi/Audio Effects`). A script you approve runs with your DAW's access, so what it writes
follows its code. Kumi reads WAV and AIFF itself; for MP3, FLAC and other compressed audio it uses an ffmpeg already
on your computer, if you allow it at setup (it never downloads one). Removing the plugin keeps `plugin-data/kumi/`;
deleting it removes everything Kumi wrote for Hermes there, including sounds from integrations that your projects may
play (see Integrations).

## Integrations (optional)

Core Kumi needs neither of these.

**Sounds from other Hermes plugins (ElevenLabs).** Kumi can measure a finished sound another Hermes plugin saved in
`<HERMES_HOME>/cache/generated/audio`, or put it in your project after you approve the change; it copies the file into
its own data first.

**Mirelo (paid).** `kumi_generate` asks Mirelo for sound effects from a prompt and for MIDI notes from an audio file.
It's off until you set all four:

| Setting | Default | What it does |
|---|---|---|
| `mirelo_api_key` | none | Your Mirelo API key, a Hermes secret (`MIRELO_API_KEY`). Kumi sends it only to Mirelo, with a generation you approved |
| `mirelo_acknowledged` | off | Lets Hermes ask for paid Mirelo generations, each approved with its estimate in Mirelo API credits |
| `mirelo_sfx_enabled`, `mirelo_midi_enabled` | off | Allow each kind: sound effects from a prompt, MIDI notes from an audio file |
| `mirelo_credit_cap` | 0 | The most Mirelo API credits Kumi may reserve, over its whole life. 0 refuses every paid generation |

In a terminal, `hermes kumi credits` shows what's spent, reserved and left. `hermes kumi credits unfreeze` lets paid
generations start again after a charge came in above its estimate, and `hermes kumi credits release <id>` releases a
reservation whose charge stayed unknown, once you've checked it on Mirelo's dashboard. Both ask for your yes in a
terminal, and no tool can run them. What leaves your computer, and how credits are reserved: see Disclosure.

**Their sounds in your projects** stay in `plugin-data/kumi/`, and the project plays them from there. Before deleting
that folder, save such projects with their files copied in (Live: **File → Collect All and Save**; REAPER: **Save
project as…** with **Copy all media into project directory**), or they'll miss those samples.

## Disclosure

- **Download:** after you approve setup, Kumi downloads the exact platform release named in this plugin's
  `kumi_release.json`, only from `github.com/user1303836/kumi/releases/download/`. It checks the SHA-256 this plugin
  pins and installs the release under this profile's plugin-data folder; setup shows the download and unpacked sizes.
- **Writes outside its data:** setup may install Live's Remote Script with its Kumi Ears device, a REAPER extension or
  a Renoise tool, only at the locations you approve. Each Max for Live device you approve is written as a new file in
  Live's User Library (`User Library/Kumi/Audio Effects`); it stays there after an undo, and in any Set that uses it.
  When a change in Live reaches its third step, or has a step that can lose material (a delete, clearing a range, or
  moving a clip onto what's there), Kumi copies the Set as last saved beside it (`<Set>.backup-<date>.als`), once for
  each saved version; an unsaved Set gets no copy.
- **Processes:** Kumi's runtime, its DAW bridge and its library workers run on the machine that runs Hermes, on
  loopback only. They stop when the chat ends, when the plugin unloads or Hermes exits, or after 15 minutes idle (2
  hours while connected to your DAW for a chat); a job still running, such as a scan or a Mirelo generation, keeps
  them up for at most 6 hours. Your DAW keeps running.
- **Reads:** the open project; the audio files in the sample folders you list (never a whole drive or volume, your
  home folder or above, or inside or above HERMES_HOME; links never lead it outside them); its renders of the windows
  you approve (REAPER and Renoise); and finished audio other Hermes plugins saved in
  `<HERMES_HOME>/cache/generated/audio` (which Kumi never creates), to place or measure it.
- **Approvals:** changes, undos, listens, play, scripts and devices go through Hermes's approval; stop never asks.
  - **Allowing for the session or always** covers that kind of call (changes, undos, listens, play, sample-folder adds
    and scans) in this chat only, including a one-shot run that resumes it (`hermes chat -q --resume`). Another chat,
    a new one-shot run or a scheduled job asks again or is refused.
  - **Only your own turns:** when Hermes works on a chat in the background (its review after a turn) or answers a
    side question (`/btw`), Kumi only reads, whatever you allowed for the session.
  - **Narrower approvals:** a setup step's approval covers its own plan. A change only your DAW's own Undo takes back
    asks under its own kind. With Mirelo, an upload's covers its one file in this chat, and a paid generation's only
    its own estimate, once.
  - **Scripts and devices:** each shows its code (a device, its controls too) before you decide. In a chat that has
    read outside content (web pages, transcripts, files Hermes fetched from the internet, another plugin's results, or
    any terminal command or code run), each is asked every time; files already on your disk, installed skills, and the
    names in your library and projects don't count. A messaging chat runs none unless `scripts_in_messaging` is on;
    scheduled jobs, webhooks, `hermes chat -q` and Hermes's API server never run one.
  - **"Always"** leaves an entry in Hermes's `command_allowlist` (the profile's config.yaml) that Kumi never edits; to
    remove it, delete the `plugin_rule:kumi:` entries there.
  - **When Hermes's approvals are off** (`--yolo`, a chat's `/yolo` or `approvals.mode: off`), Hermes runs all of these
    without asking (Mirelo's up to your credit cap), and `/kumi` says so; scripts and devices run unasked only in a
    chat that hasn't read outside content, and are refused in one that has. A one-shot `hermes -z` turn is yolo too,
    and Kumi can't tell it apart.
- **HISTORY and undo:** HISTORY records what actually happened, including when an outcome is uncertain. Kumi can undo
  a change only while the runtime that made it is still running; after that, use your DAW's own Undo. In Live, an undo
  refuses to touch a value you've changed by hand since; in REAPER and Renoise, it puts the value back anyway. A few
  changes only the DAW's own Undo takes back: in Live, adding a chain to a rack, moving a device and loading a sample
  onto a pad, each a change of its own, with how Cmd+Z (Ctrl+Z) takes it back said before you approve; and a script,
  in any DAW.
- **Listening:** Kumi measures audio files on this computer, and can listen to a track or the mix: after you approve,
  only that window (60 seconds at most, with the DAW stopped) is heard. REAPER and Renoise render it into the
  listening folder of Kumi's part in that DAW, and Kumi copies that one file into its data. The render's temporary
  transport, routing and device settings are put back after; REAPER may show the project as modified, and Renoise's
  BPM and lines per beat are put back only if they're still what the render left. In Live (256 beats at most, in the
  Arrangement), Kumi captures it through Kumi Ears, after each track's devices and before its fader, then removes the
  device; Live may show the Set as modified, and its own Undo history holds the listen's temporary device steps
  (pressing Undo in Live can bring Kumi Ears back).
- **Playback:** play asks first and never starts a play that would record; stop never asks. Neither changes anything
  in the project but where play starts.
- **Scripts:** a script you approve runs inside your DAW with the DAW's access to this computer (its files and
  network). Kumi refuses one over 400 lines or 32 KB, with hidden or direction-changing characters, or using process
  or network calls it recognises; that check is a safeguard, not a sandbox. Kumi stops a script after 10 seconds (0.8
  in Renoise).
- **Project ids:** Kumi's REAPER extension keeps an id in a saved project, and its Renoise tool keeps one in a song,
  only when a standalone Kumi connected to that DAW asks it to; Kumi for Hermes never asks. Once kept, the id is saved
  with the project.
- **ffmpeg:** with your approval at setup, Kumi's runtime runs an ffmpeg already on this computer to decode compressed
  audio. Kumi never downloads one, and checks the ffmpeg file you allowed before each start (not the libraries it
  loads, which an upgrade can change). It hands ffmpeg only the one file it's reading, never a URL or a playlist, and
  ffmpeg reads nothing from the network.
- **What reaches a model:** audio is measured on this computer and never sent to a model; in a messaging chat
  (Telegram, Discord…), a sound or measured audio you ask Hermes to send you goes to that chat as an attachment,
  through that app. The project details Kumi notes for FOCUS, and the measurements it returns, go to the model you use
  in Hermes as part of the conversation; Kumi's status names no folder on this computer to it. With
  `listen_interpretation` on, Kumi also sends the measurements and a small image of the measured band energy (never
  the audio or file paths) through Hermes's own model access: to the model set for Kumi's listening task in Hermes (it
  must accept images), or to the fallback providers Hermes is configured to try for that task. Hermes retries a failed
  attempt and may fall back, which can add time and token cost.
- **Another Kumi's installation:** if you let Kumi for Hermes use another Kumi's installation (Live only), its runtime
  reads that installation's DAW connection files and writes its own connection state there, never its sign-in or
  settings. A standalone Kumi's sign-in, model providers, account files and updater are never used.
- **Scheduled jobs** (Hermes cron) read only Kumi's status and HISTORY as last published; a job's answer (change
  titles, track names) leaves this computer only through where that job delivers, and Kumi's job blueprints deliver
  locally by default.
- **Mirelo** (off until you add a key, acknowledge it, turn a kind on and set a cap above 0): your prompt, and for MIDI
  the audio file you approve, go with your key to `api.mirelo.ai` and to the upload and download links Mirelo's API
  gives for each job, on hosts Mirelo chooses (its storage or delivery network), so they aren't a fixed list. Whether
  Mirelo trains on what you send follows your Mirelo account's settings; Kumi has no setting of its own for it.
  - **Credits, never money:** each generation is approved with Mirelo's estimate in API credits and reserves up to it
    against a lifetime cap (0 by default, which refuses every paid generation); sending a file is approved on its own.
    A charge may stay unknown until Mirelo reports it, and its reservation keeps counting against the cap meanwhile. A
    charge above its estimate pauses new paid generations.
  - **Yours alone, in a terminal:** letting paid generations start again (`hermes kumi credits unfreeze`) and
    releasing a reservation whose charge stayed unknown (`hermes kumi credits release <id>`).
  - **Never unattended:** nothing is paid for or uploaded in scheduled jobs, `hermes chat -q`, webhooks or Hermes's
    API server.
  - What Mirelo makes is saved in this profile's plugin data, and placing it in your project is a separate approved
    change. Switching a kind off stops new jobs (one already running is still fetched while the key is set); removing
    the key stops Kumi contacting Mirelo.
- No telemetry.

## Compatibility

- Hermes 0.22.7 or later.
- Ableton Live 12.4.15b4 (beta) or later on macOS and Windows; REAPER 7 or later and Renoise 3.5 or later on macOS,
  Windows and Linux.
- The DAW must run on the machine that runs Hermes: a Desktop app connected to a remote Hermes controls that
  machine's DAW, not its own.

## Development

The plugin lives in Kumi's repository at `hermes-plugin/`. Its tests need no DAW, network or sign-in:

```sh
python3 -m unittest discover -s hermes-plugin/tests
hermes plugins validate hermes-plugin
hermes plugins doctor hermes-plugin --ci
```

A Kumi release is pinned with `python3 scripts/hermes_pin.py <version>`, which records each platform's bundle,
SHA-256 and sizes after checking them against the release's own records.

## License

MIT, as the rest of Kumi.
