# Kumi for Hermes

![Kumi for Hermes](../docs/assets/hermes/banner.png)

**Drive Ableton Live, REAPER and Renoise from Hermes.** Kumi reads the project you have open, makes typed changes
in it after you approve each one, records every change in an undoable HISTORY, measures audio on your computer, and
maps the sample folders you choose so you can find, audition and place sounds. Hermes stays the agent: Kumi gives it
a DAW to work in.

- **Ask in plain words, from any Hermes surface.** "Put a darker snare on pad 2", "bring the bass up 2 dB",
  "undo the snare change". The TUI, Hermes Desktop, the web dashboard and messaging all use the same tools.
- **Every change is approved and reversible.** Hermes asks before Kumi changes your project, shows exactly what will
  change, and Kumi records the result in its HISTORY, with undo for as long as the change can still be undone.
- **Kumi knows what you're looking at.** While a chat is connected, each turn carries a short note of the track,
  device and clip you have selected (FOCUS), and what changed since Kumi last looked.
- **Your own sounds, on a map.** Kumi learns the sample folders you list and lays them out by how they sound; in
  Hermes Desktop the map opens in its own tab, where a click auditions a sound and "Use" asks Hermes to place it.
- **Measured, not imagined.** Loudness, true peak, loudness range, spectrum and tempo are measured on your
  computer. Kumi never claims to have heard what it hasn't measured.

## Tools

| Tool | What it does | Asks first |
|---|---|---|
| `kumi_status` | Setup, runtime and DAW connection state | no |
| `kumi_setup` | Plans and installs Kumi's runtime and its part in your DAW | yes, each step |
| `kumi_connect` | Connects to the DAW chosen in settings | no (reads only) |
| `kumi_observe` | A short, current look at the project: FOCUS, what changed, the project overview | no |
| `kumi_read` | Bounded reads of tracks, devices, clips and their settings | no |
| `kumi_change` | One typed change of one or more steps, recorded in HISTORY | yes |
| `kumi_undo` | Undoes a HISTORY entry while it can still be undone | yes |
| `kumi_history` | Lists recent changes and whether each can be undone | no |
| `kumi_listen` | Measures an audio file, a library sound or a result Kumi made | no |
| `kumi_sounds` | Searches your sound library | no |
| `kumi_library` | Adds a sample folder and scans it | yes |
| `kumi_job` | Reports a long job (a library scan) | no |

Three skills teach Hermes how to use them well: `kumi:daw`, `kumi:sounds` and `kumi:listening`.

## Install

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

In Hermes Desktop, also switch on Kumi's Desktop half under **Capabilities → Plugins**: plugins' Desktop halves start
switched off.

## Set up

Kumi needs two things on the machine that runs Hermes, and asks before each:

1. **Its runtime:** Kumi's official release for your platform, 25–32 MB, from
   `github.com/user1303836/kumi/releases/download/v<version>/`. Kumi checks it against the SHA-256 this plugin pins,
   unpacks it (77–113 MB) into this profile's plugin data, and refuses anything that doesn't match.
2. **Its part in your DAW:** Live's Remote Script, REAPER's extension or Renoise's tool, installed only at the location
   shown to you. If another Kumi installation already put it there, setup offers to use that installation instead of
   replacing it.

Ask Hermes to "set up Kumi for Live" from any surface, or run `hermes kumi setup` in a terminal. Then, in the DAW:

- **Live:** choose **AbletonMcpBridge** as a Control Surface in Settings → Link, Tempo & MIDI.
- **REAPER:** restart REAPER.
- **Renoise:** enable the Kumi tool.

Setup never starts or quits your DAW, picks a Control Surface for you, or saves a project.

## Settings

Under Settings → Plugins → Kumi in Hermes Desktop, or `plugins.entries.kumi.settings` in `config.yaml`:

| Setting | Default | What it does |
|---|---|---|
| `daw` | Live (`ableton`) | Which DAW Kumi connects to |
| `library_folders` | none | The folders Kumi may read to build your sound map. Nothing else is scanned |
| `inject_focus` | on | Adds the short FOCUS and catch-up note to each turn while Kumi is connected |
| `listen_interpretation` | off | Also asks your Hermes model to interpret measurements (see below) |

## Hermes Desktop and the dashboard

- **Desktop:** a status chip (DAW, project, tempo), a side pane with FOCUS and HISTORY, the sound map in its own tab,
  cards for measured audio, and palette commands. Buttons never change your project directly: they write a request
  into the chat, which goes through the same approval.
- **Web dashboard:** a Kumi tab with the same HISTORY, status and map.

## Files

Everything Kumi writes for Hermes stays in this profile's `plugin-data/kumi/` folder: the runtime, the library index
and map, HISTORY, measurements and job records. Outside it, setup writes only Kumi's part in your DAW, at the location
you approved. Removing the plugin keeps `plugin-data/kumi/`; delete it to remove everything.

## Disclosure

Kumi downloads the exact platform release named in this plugin's `kumi_release.json`, only from
`github.com/user1303836/kumi/releases/download/`, after you approve setup. It verifies the SHA-256 this plugin pins
and installs under this profile's plugin-data folder; setup shows the download and unpacked sizes. Setup may install a
Live Remote Script with its Kumi Ears device, a REAPER extension or a Renoise tool, only at the locations you approve.
Kumi's runtime, its DAW bridge and its library workers run on the machine that runs Hermes, on loopback only, and stop
when the chat ends, when the plugin unloads, or after 15 minutes idle; your DAW keeps running. Kumi reads the open
project and the sample folders you list. Changes to your project go through Hermes's approval, and HISTORY records what
actually happened, including when an outcome is uncertain; undo is available only while the recorded change can still
be reversed. Listening to a track or the mix also asks first: it plays only the beat window you approve (60 seconds at
most), only while the DAW is stopped, and may change routing and transport briefly, restoring them after. Live captures
it through Kumi Ears (Max for Live); REAPER and Renoise render it into the listening folder of the Kumi installation
that owns their component, and Kumi copies the file it asked for and removes exactly that file. Audio is measured on
this computer and not uploaded. The project details Kumi notes for FOCUS, and the measurements it
returns, go to the model you use in Hermes as part of the conversation. With `listen_interpretation` on, Kumi also
sends the measurements and a small image of the measured band energy (never the audio or file paths) to your Hermes
model through Hermes's own model access, using the model you choose for the `kumi_listen` auxiliary task, which must
accept images. If you let Kumi use an existing Kumi installation, its runtime reads that installation's DAW connection
files, never its sign-in. Kumi's own sign-in, model providers, account files and updater are never used. No telemetry.

## Compatibility

- Hermes 0.22.7 or later.
- Ableton Live 12 on macOS and Windows; REAPER 7 or later and Renoise 3.5 or later on macOS, Windows and Linux.
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
