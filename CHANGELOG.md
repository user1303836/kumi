# Changelog

Kumi's releases. The Ableton bridge (`crates/ableton-mcp-server`) is versioned on its own;
each Kumi release names the bridge it ships with.

## Unreleased

### Bridge 1.0.75

- Live's own timer serves Kumi between Live's display ticks, so a look at the Set takes about 20 ms
  rather than about 100, and a plan's changes don't each wait for a tick. Kumi's reads keep to the
  same share of Live's main thread as before; a change takes what Live takes to make it.

## 1.8.1 — 2026-10-05

Ships with bridge 1.0.74.

- Setup takes one command. The one-line installer starts Kumi when it finishes (`KUMI_NO_LAUNCH=1`
  skips it). On first run Kumi shows the steps still missing, in its own window:
  - signing in;
  - putting its bridge in Live. An open Live is asked to quit; it asks to save first, then opens again
    by itself.
  - choosing AbletonMcpBridge as a Control Surface.

  Esc chats without Live for now.
- In a chat without Live, Kumi connects by itself once Live answers, so `/reconnect` is no longer
  needed.
- Searches and reads that don't touch Live run at the same time, up to four at once: the web, sounds,
  presets, your Sets, the Live manual and earlier conversations.
- When an answer breaks off partway (a dropped connection, say), Kumi carries on once from where it
  stopped, and the status line says so.
- A conversation keeps its provider's prompt cache across restarts: a resumed conversation's first
  answer reads most of its prompt from the cache.

Tested with Live 12.4 on macOS. On Windows, installing and updating are tested; using Kumi with Live
there is still new.

## 1.8.0 — 2026-10-05

Ships with bridge 1.0.74.

- Stopping Kumi partway through a batch of changes keeps the ones that finished in the
  conversation, so the next request doesn't redo them. The change that was cut off is marked to
  check in Live.
- In long tasks such as matching a sound or a goal, when the earliest exchanges are dropped to make
  room, what you said in them stays at the start of the conversation. An early "don't touch the
  drums" keeps holding.
- When Kumi asks you to pick (which track, which version), its options appear above the input box.
  Press a number and Enter, or type your own answer.
- When the provider is busy or a connection drops, NOW says why and counts down to the next try
  ("retrying in 5s · ChatGPT is busy (HTTP 429)"). Esc still stops.
- `/fast` turns on the model's faster tier when its provider offers one: ChatGPT's "Fast" answers
  sooner and uses more of your plan. It shows as "· fast" beside the model.
- `/recipe <name> blank=value …` runs a saved recipe straight away, without a model call. Values with
  spaces go in quotes. In `/recipes`, "Run it on…" starts that line with what's pinned filled in.
- Show Kumi pictures and files: drag them into the window, or press ctrl+v for a picture on the
  clipboard. PNG, JPEG, GIF and WebP pictures up to 3.75 MB each (20 MB a message) go to the model
  beside your words, and every file also goes by its path. Saved conversations name each picture
  instead of keeping it.
- Notes can be pinned: a full memory drops its oldest unpinned note. In `/memory`, a note's words
  can be changed, and the note pinned, unpinned or forgotten (`/note`, `/pin` and `/unpin` in plain
  mode).
- Kumi can search earlier conversations in every Set by their words, with what they changed and the
  kept techniques and recipes.
- Every tool result is capped at 64 KB, keeping its opening and saying how to see more, so one huge
  read no longer overflows a request.
- Kumi notes where each answer's time went (model, tools, Live requests, bytes sent) in
  `~/.kumi/timings.jsonl`, and `kumi report` includes a summary.
- The source tree is Rust only: the TypeScript app, runtime and bridge are gone (they stay at the
  v1.7.6 tag). `npm run kumi` still builds and runs the checkout with Cargo, as in 1.7.6.

Tested with Live 12.4 on macOS. On Windows, installing and updating are tested; using Kumi with Live
there is still new.

## 1.7.6 — 2026-10-04

Ships with bridge 1.0.74.

- Kumi and its bridge run as native Rust executables. Fresh installs need no Node runtime.
- Kumi opens in about half the time, small changes in Live are 2–3× faster, and library learning is
  1.7× faster on about a sixth of the memory.
- Current 1.7.4 and 1.7.5 installer users keep using `kumi update`. The one-time update downloads
  only their platform's build. Settings, sign-ins, conversations, library data and the configured
  Kumi home stay in place.
- The existing JavaScript bridge switches to the native bridge the first time Kumi starts with Live
  closed, including when both are version 1.0.74. Until then Kumi works through the Remote Script
  already in Live; coming from 1.7.5, whose Remote Script is the same, nothing is shown. The
  bridge's secret, ports and configuration paths stay.
- On Windows, the first native start replaces the Node launcher.
- `kumi update --rollback` restores the previous app and its bridge generation. Close Live before
  rolling back to the JavaScript app. Another rollback returns to the retained native app.
- YouTube downloads reuse the managed Node runtime retained from an older installation for
  yt-dlp’s JavaScript challenges, even when Node is absent from PATH.
- Existing npm commands build the current checkout when Cargo is installed; otherwise they hand
  off to a published native release using the same Kumi home.

Tested with Live 12.4 on macOS. On Windows, installing and updating are tested; using Kumi with Live
there is still new.

## 1.7.5 — 2026-10-04

Ships with bridge 1.0.74.

- Python that removes notes the pre-Live 11 way (`remove_notes`, `replace_selected_notes`) no longer
  stops Live with "A custom MIDI Remote Script uses an older process to modify MIDI notes". The bridge
  (1.0.74) refuses those calls before a script runs and names Live 11's calls instead. Kumi's model is
  also told to use Live 11's calls, which keep each note's MPE, probability and velocity data.

## 1.7.4 — 2026-10-03

Ships with bridge 1.0.73.

Kumi gets to the answer in fewer model replies, which are most of a request's time (each takes
several seconds). Across the change eval's 22 requests with gpt-6.1-sol, model calls went from 84 to
70; building a sound from a tutorial went from 10 calls to 5–7, and "make the bass a bit quieter"
from 2 to 1.

- Building a chain: devices are loaded and their parameters set by name in one plan, with values as
  Live shows them, without reading the parameters first. A parameter name a device doesn't have, or a
  value it can't take, no longer stops the plan: the rest runs, and the result lists what was missed
  with every parameter each device has, so one more plan fixes them all.
- Each turn's look at the Set has every track's level and pan (on a Set of up to 64 tracks), so
  "make the bass a bit quieter" is one reply instead of two.
- After a device is moved or deleted, the result lists its track's devices as they are now, instead
  of asking the model to read them again.
- Asking Kumi to undo something ends with Kumi's own reply ("Undone: …"), without a model reply after it.
- `make_device` says where it wrote the device's file, so working on the device later starts there
  instead of searching for it.
- The change eval (`npm run eval:changes`) counts each case's model calls and its tools' time, takes
  `EVAL_EFFORT`, and has every parameter of Live's Operator, Saturator and EQ Eight; its bridge tools
  are up to date with the bridge's.

## 1.7.3 — 2026-10-03

Kumi says plainly when a device it's working on was deleted in Live. Ships with bridge 1.0.73, as 1.7.0, 1.7.1 and 1.7.2 did.

- A device deleted in Live while Kumi works on it: setting one of its knobs again now says the
  device isn't in Live any more, instead of Live's own C++ error, and undoing a change on it says
  the same, instead of that the knob "changed in Live since".

## 1.7.2 — 2026-10-03

Kumi turns a device's knobs several times faster. Ships with bridge 1.0.73, as 1.7.0 and 1.7.1 did.

- Kumi sets a device's parameters in one request to Live instead of five. Each request waits for
  one of Live's display ticks, so nine parameters on three devices (a rack from a tutorial, say)
  went from 4.2 to 1.7 seconds on real Live, 0.7 seconds when set again (as in matching a sound),
  and their undo from 3.2 to 0.9 seconds. Kumi's own Python sets them inside Live, where the
  bridge runs Python (1.0.68 and later); HISTORY reads as before. `KUMI_FAST=0` goes back to the
  bridge's preview and apply.
- Undoing a parameter change now leaves a parameter you turned since where you put it, and says
  which, instead of putting it back anyway.

## 1.7.1 — 2026-10-03

Kumi's commands show they're working, and `kumi bridge` gets past its first line in moments on
Windows. Ships with bridge 1.0.73, as 1.7.0 did.

- `kumi bridge`, `kumi update`, `update --check`, `uninstall`, `doctor`, `report` and `login`
  show a small spinner after each step's line while it runs ("Updating Live's Remote Script and
  the bridge… ⠹"), so a step that takes a minute doesn't look stuck. The line stays once the
  step is done. Piped output, `KUMI_UI=plain` and `TERM=dumb` get the plain lines, as before.
- On Windows, `kumi bridge` and `kumi update` sat on "Updating it takes a minute." for over a
  minute on some computers before asking whether Live is closed: `tasklist`, which Kumi asked
  whether Live was running, took 78 seconds on one. Kumi now asks PowerShell's `Get-Process`,
  which answers in about a second, and falls back to `tasklist` only when PowerShell won't start.
- Running from a checkout on Windows with Node 24, packing the bridge no longer prints Node's
  DEP0190 warning.

## 1.7.0 — 2026-10-02

Kumi hears what it works on, uses Live's own commands that its scripting lacks, works inside ten
popular plug-ins, knows your library, listens when you talk, runs on models on your computer and
turns loops into arrangements. Ships with bridge 1.0.73.

### Hearing the Set

- Kumi Ears, a small Max for Live device Kumi installs in your User Library, hears any track,
  return or the whole mix: quietly over a part while Live is stopped, or as it plays. `listen`
  hears a track, several tracks (and what clashes between them) or the mix; auditions use it
  instead of recording. The device comes and goes with each listen. Without Max for Live, Kumi
  records to listen, as before.

### Live's own commands

- Group, ungroup, freeze, unfreeze, flatten, bounce, consolidate, convert to MIDI, separate
  stems, slice, save and export, through Live's menus. Tracks are selected by name through Live
  12's accessibility, so Live stays where it is. macOS asks once for Accessibility.

### Plug-ins

- Guides for Serum 2, Vital, Ozone 12, Pro-Q 4, Pro-L 2, Saturn 2, Decapitator, OTT, Supermassive
  and Pigments: what each does, its real parameters, and which ones Kumi can turn now.
- Values in a parameter's own units: "800 Hz", "-6 dB", "Saw".
- Wavetables Kumi makes, for Serum, Vital and other wavetable synths.

### Library

- Kumi learns your sounds, presets and Sets in the background and finds them by what they are
  and how they sound (`find_sounds`, `find_presets`, `my_sets`). `/memory` shows the habits it
  learned from your Sets; `kumi library` shows how far it's got.

### Talking

- Ctrl-T talks instead of typing. What you say is written down on your computer (whisper.cpp);
  `/voice` sets the language and the microphone.

### Models on your computer

- Ollama, LM Studio and any OpenAI-compatible server, found by themselves, with no sign-in.
  `kumi doctor` names them and how to start one that's closed.

### Arrangements

- `arrange` lays out an arrangement from your Session scenes or a loop: sections with locators,
  gaps, fills and risers, as one change with one undo.

### Bridge 1.0.73

- A Max for Live device's load is confirmed: its ins and outs read the same every time.
- Audio effects load onto Main and the return tracks.

## 1.6.1 — 2026-10-02

Kumi on Windows, from installing it to its first answer: `kumi update` works from any PowerShell,
`kumi bridge` notices Live within moments and handles a User Library on another drive, Kumi's Live
extension goes where Live on Windows looks for it, and Windows terminals show Kumi's colours and
icons. With the optional Willington provider, one installation covers Live 12.4.15b4 and b5, and
rack chain zones are supported on b5. Ships with bridge 1.0.72.

### Windows

- `kumi update` stopped at "Unpacking it failed: tar: Error is not recoverable: exiting now" in a
  PowerShell started from Git Bash: Git's GNU tar came first on PATH, and it reads "C:\…" as a
  remote host. Kumi now runs Windows' own tar, as its installer always has, and so does fetching
  yt-dlp, ffmpeg and whisper.cpp for videos (GNU tar opens no zip either). PowerShell and tasklist
  are run by their full path too.
- From 1.6.0 or earlier in such a window, run the installer line once, or put Windows' own folder
  first in that window and update:
  `$env:Path = "$env:SystemRoot\System32;$env:Path"; kumi update`
- Kumi's Live extension goes in `%LOCALAPPDATA%\Ableton\Extensions`, where Live on Windows keeps
  it (and its database). 1.6.0 put it in `%APPDATA%\Ableton`, where Live never started it;
  `kumi bridge` moves it, and the update runs that for you.
- The Windows console and Windows Terminal get Kumi in 24-bit colour (it took them for 16
  colours), and WezTerm and Git Bash's own window get its icons rather than two-letter badges.

### Setting up the bridge

- While `kumi bridge` waits for Live, it looks every 2 seconds whether Live's Remote Script
  answers, and checks the connection in full only then. It notices Live within a few seconds;
  each full check took several seconds on Windows.
- Enter or Ctrl-C stops the waiting. It's read as a key, so a step under way finishes, and on
  Windows cmd doesn't ask "Terminate batch job (Y/N)?" afterwards.

### Optional Willington provider

- Willington's multi-version bundle picks each provider's bindings for the Live build that's
  running, so one installation covers macOS ARM64 Live 12.4.15b4 and b5. It needs
  `WillingtonRuntime` beside the providers; see
  [Optional Willington integration](docs/en/WILLINGTON_INTEGRATION.md).
- Rack chain zones are supported on Live 12.4.15b5, checked in real Live with 42 signal-gating
  checks, 49 fade measurements and seven Max `live.object` write, read and restore cycles
  ([summary](docs/evidence/rack-zones-b5.json)).
- A provider with no bindings for the running build is skipped on its own: on b4, Follow Action
  and rack macro edits keep working without zones. The bridge tries a skipped provider once per
  Live session and logs why once. Missing files or a failed integrity check still turn all of
  Willington off, and Kumi carries on without it.
- Follow Action writes check the self-test receipt against the library picked for this build.
  After switching builds, run the self-test again; the guide gives its steps for the bundle.
- Live's log names the Willington providers that started and those with writes on.

### Bridge 1.0.72

- The Willington changes above.
- A User Library on another drive than Kumi's folder (D:, an external drive) upgrades, rolls back,
  repairs and uninstalls. Those moved the Remote Script's folder with a rename, which can't cross
  drives; now it's copied, and its reference file is made owner-only again.
- A lifecycle lock left by a process that has gone (a `kumi bridge` stopped mid-step) no longer
  refuses every later install, upgrade and uninstall.
- On Windows each owner-only file is set and checked in one PowerShell instead of two, which takes
  about a quarter off each install, upgrade and uninstall step. The Remote Script runs PowerShell
  by its full path, and looking for Live's Extension Host no longer holds up the bridge.

## 1.6.0 — 2026-10-02

Kumi works with Live on Windows: the bridge installs when Live's User Library is outside your user
folder, Kumi starts it, and its Remote Script loads in Live, whose own Python has no `ctypes`.
Ships with bridge 1.0.71.

### Bridge 1.0.71

- On Windows, `kumi bridge` installs into a User Library outside your user folder (another folder
  on C:, another drive). It stopped there with "could not establish an owner-only Windows ACL":
  making a file yours alone also set its owner, which Windows lets an ordinary account do only
  where it has full control.
- On Windows, the bridge runs PowerShell by its full path. Kumi starts the bridge with a PATH that
  holds only Node's folder, so the bridge couldn't check its secret's permissions and refused to
  start, leaving Kumi without Live.
- On Windows, the Remote Script's owner check no longer needs `ctypes`, which Live's own Python
  comes without, and its permission checks no longer open console windows.

## 1.5.0 — 2026-10-02

With the optional Willington provider installed, Kumi edits Session clip Follow Actions, rack macro
names and mappings, the selected variation's name and rack chain zones, each with a preview, undo
and a refusal when Live changed in between. Ordinary Kumi needs nothing new. Ships with bridge
1.0.70.

### Optional Willington edits

- Willington is a separately installed native provider for one exact Live build (macOS ARM64,
  Live 12.4 beta). Its tools appear only once it is set up and its writes enabled; see
  [Optional Willington integration](docs/en/WILLINGTON_INTEGRATION.md).
- `set_clip_follow_actions` sets all ten Follow Action fields of a Session clip (actions, chances,
  linked or unlinked timing, loop count, jumps) while playback is stopped.
- `edit_rack_mapping` renames a macro or the selected variation, and maps or unmaps a parameter on
  a macro with a continuous, enum or boolean range, inverted where the parameter allows.
- Rack chain zones with fades: selector zones on Audio Effect, Instrument and MIDI Effect Racks;
  key and velocity zones on Instrument and MIDI Effect Racks.
- Only the edit kinds the installed provider supports are offered. A missing Follow Action
  self-test turns off Follow Action writes alone, and Live's log says why.
- `willington.json` beside the installed bridge survives Kumi updates, with its permissions.

### Bridge 1.0.70

- The Willington operations above, offered only when the provider is present.
- Racks read with Live's own macro layout; a chain lists its mixer parameters for mapping.
- Recalling or deleting a rack variation by index happens in one step in Live.
- An empty or missing Modulators category in Live's Browser falls back to the stock modulator
  devices under Audio and MIDI Effects.
- `set_clip` sets Legato and is offered before the Set has clips; a MIDI clip made earlier in a
  plan can be used by later steps.
- Note batches whose values Live rounds (0.1, triplets) no longer fail their check, in any order.

## 1.4.0 — 2026-10-02

You can see what Kumi is doing and talk to it while it works: each step animates by kind, repeats
fold into one line, Enter tells Kumi more mid-answer, Tab queues a message, `/btw` asks on the side,
and a yellow light blinks on Live's beat. Nothing is cut from the conversation. Ships with bridge
1.0.69.

### Seeing and talking to Kumi while it works

- Each step has its own animation while it runs (searching, reading a page, looking at the Set,
  building a device, changing, listening, watching, playing, recording, code), its words shimmer
  and its time counts up; NOW plays a wider version of it.
- The same step done several times in a row folds into one line ("read a page ×3") 3 s after the
  last, with a short fold.
- Enter while Kumi works sends a message it reads after the step under way; Tab sends one for after
  the answer. Waiting messages show above the box; Alt-↑ takes the last one back, and stopping
  Kumi puts them back in the box.
- `/btw` asks something on the side, any time: answered from the conversation so far, without
  tools, in a panel; neither the question nor the answer joins the conversation.
- While Live plays, a yellow light in the header blinks on its beat beside the tempo.
- Nothing is cut from the conversation: answers keep their steps, a conversation brought back comes
  back whole with its steps, and long titles wrap. Ctrl-Home and Ctrl-End go to the start and back.

### Bridge 1.0.69

- `run_python`'s timeout stops a tight loop on Live's Python 3.11 (`while True: pass` sent no
  trace events there, so the script could hold Live); the script's own code is traced per
  instruction, the clock read every 64.

## 1.3.0 — 2026-10-01

Kumi does what you ask and finds a way when no tool fits; the bridge refuses only what Live can't
do or what would act on the wrong thing. Ships with bridge 1.0.68.

### Doing what's asked

- Kumi acts without asking first, picks the likeliest reading of a vague request, and says
  afterwards what it chose. When no tool does exactly what's asked it takes another route (a plan
  of several, a recording, a render, a device it makes) and says it couldn't only after trying.
- It plays, launches, selects and shows things whenever that helps, and deletes what a request
  implies (replacing, cleaning up, starting over).
- Any audio file on your computer loads by its path, not only one Kumi found.
- Max for Live devices get as many knobs as they need (up to 128, in rows past eight), longer
  menus, up to 32 voices, longer code, feedback that sustains or self-oscillates, and MIDI
  effects that run on their own (LFOs, clocks, generators).
- Longer answers: up to 200 steps, 60 minutes, 10 minutes of quiet thinking, 5,000 changes, waits
  of 30 minutes; three retries when a provider is busy. Matching only starts when there's
  something to match.
- Kumi runs on Node 25 and newer, not only 22 and 24. Notes past the 24 kept forget the oldest.

### Bridge 1.0.68

- `run_python` runs Python inside Live for APIs the typed tools do not cover: eval returns an
  expression; exec returns `result`. It captures stdout and error details, returns usable Live
  object refs, and checks a timeout. Each run uses one Live undo step and clears cached Set reads;
  scripts have no HISTORY entry. MCP clients can call `live_run_python`.
- Refusals say why, with Live's own reason, instead of "requires fresh authoritative state".
- Undo takes back the change on the same object however it changed since: a renamed track, a
  device with clips added, a knob or fader moved again.
- A clip launches while the Set plays; recording starts with other tracks armed too, or already
  on; output-safety evidence is optional.
- Tracks and scenes may share names; random MIDI transforms pick a repeatable seed themselves;
  a value past a parameter's range or between its steps goes to the nearest it takes; a
  switched-off device's knobs can be set.
- Long sessions never run out of room for new changes: the oldest applied one gives up its undo.

## 1.2.0 — 2026-10-01

Kumi takes full control of Live and stays quick on big Sets. It deletes what you ask for, writes MIDI
straight into the Arrangement, renders offline, and answers for what you point at in Live. A 200-track
Set answers about as fast as a small one. Web search no longer leans on one free service. Ships with
bridge 1.0.66.

### Full control of Live

- `kumi bridge` puts Kumi's own Live extension into Live's Extensions folder, and Live runs it.
  `kumi doctor` says whether it's running, and `kumi uninstall` takes it out again. Through it:
  - MIDI with its notes goes straight into the Arrangement (`write_arrangement_clip`).
  - A stretch of a track's Arrangement can be cleared (`clear_range`).
  - A device can be copied with its settings (`duplicate_device`).
  - `render` renders an audio track's clips offline in a fraction of a second, without playing the Set:
    the clips' own audio, before the track's effects.
  - A sample goes onto a Drum Rack pad without the Browser.
- **Right-click to point.** Right-click a track, clip, scene or Arrangement selection in Live, then
  Extensions › "kumi: Ask Kumi about this". Kumi pins it, and "this" in your next message means it.
- **Deletions.** Clips, scenes, tracks, locators, devices and returns go when you ask for it. HISTORY
  keeps what only Live's own undo can bring back.
- **A plan is one step in Live's undo.** One Cmd-Z takes back everything the plan changed through Live's
  scripting.
- **More of each device.** Settings beyond knobs (Roar, Shifter, Spectral Resonator, Hybrid Reverb,
  CC Control, Simpler's slices and warping, Wavetable's modulation), a plug-in's every parameter by
  name, and a clip's automation. Live's own undo and redo are there for what you did in Live.
- **FOCUS follows your selection in Live** the moment it changes.

### Big Sets

- No limit on a Set's size. Kumi reads a big Set a page at a time, each page short enough that Live
  never waits on it, and folds what the model sees each turn to about the size of a small Set's.
- **One change takes about 20–120 ms** on 19 tracks or on 200. It took half a second or more before,
  and seconds on big Sets. Live's own work, such as adding a track or the moment after a rename, still
  costs what it costs when you do it yourself.
- Kumi's catch-up snapshot of a 200-track Set takes about 11 s in the background, and nothing waits
  behind it.

### Looking things up

- **Web search takes turns among free search services** (Exa, Parallel, Keenable, Firecrawl), the
  way Hermes Agent does:
  - a busy one hands the search on and rests;
  - DuckDuckGo answers when none of them can;
  - the same search within 20 minutes isn't made again.
- Pages Kumi can't read itself go through those services' readers in turn.
- An address that carries a key or token isn't read.
- When nothing answers, Kumi says why and when to try again, or asks whether the computer is online.

### Fixes

- **Undoing a change goes only to what it was made on.** Before, undoing a mixer change (and 21 other
  kinds) after tracks had moved could change another track. This was so in 1.0 and 1.1 too.
- An undo whose check failed once could skip its checks on the next try and change what you'd edited
  since. This was so in 1.0 and 1.1 too.
- After an install, `kumi bridge` sees Live connect, instead of waiting ten minutes and saying it hadn't.
- Copying an instrument beside itself is refused plainly (a chain holds one instrument). Before, Live's
  refusal left the change uncertain.
- A big message from the bridge no longer closes Kumi's connection. Past 2 MiB it used to.
- Watching a big Set names lookalike tracks once, with how many there are.

## 1.1.0 — 2026-09-30

Kumi installs with one line and keeps itself up to date, makes audio effects and
instruments, and looks things up on the web. Ships with bridge 1.0.53.

### Installing and updating

- One line installs Kumi with its own Node: no admin rights, git, npm or Node
  needed. `curl -fsSL …/install.sh | sh` on macOS, `irm …/install.ps1 | iex` in
  Windows PowerShell. Each download is checked against its checksum, running it
  again repairs or updates, and your own files in `~/.kumi` are never touched.
- `kumi update` fetches the newest release, checks it and starts it before
  swapping it in, and keeps the one before (`kumi update --rollback`).
  `kumi update --check` only says whether there's a newer Kumi.
- `/update` does the same from inside Kumi: it asks first, closes Kumi, updates
  it and opens it again, and the Set's conversation carries on. A newer Kumi is
  mentioned on the welcome screen, checked at most once a day in the background;
  `"updateCheck": false` in `~/.kumi/settings.json` (or `KUMI_NO_UPDATE_CHECK`)
  turns that off.
- `kumi uninstall` removes Kumi, its Node, its launcher and its PATH lines,
  offers to take the bridge out of Live (and keeps what Live needs while Live
  still loads it), and keeps your files unless you add `--all`. `KUMI_HOME`
  moves everything Kumi keeps.
- The installer adds Kumi to PATH the way each shell reads it: bash's existing
  startup file (never a new `.bash_profile` that would hide `.profile`), zsh's
  `ZDOTDIR`, fish for each session, and on Windows the user PATH with its
  `%VARIABLES%` kept.
- `kumi bridge` needs no npm: the bundle carries the bridge ready to install.
- A release that isn't there yet says so, rather than "check your internet
  connection".

### Making devices

- `make_device` makes audio effects and instruments too, not just MIDI effects.
  The model writes the sound in GenExpr (the language of Max's gen~), and Kumi
  builds the device around it: an effect gets Mix and Output knobs, an
  instrument up to 8 voices, each with its note, velocity, bend and mod wheel.
- Kumi's output stage keeps the device's own output safe (no NaN, denormals or
  DC, held under +6 dBFS). On an effect the dry signal passes untouched, so at
  Mix 0 the track sounds exactly as it did without it (checked on real Live).
- Kumi checks the code before it builds the device: outputs assigned, brackets
  paired, inputs read by an effect and not by an instrument, and no name gen~ or
  Kumi already uses (a control's Param declared again included).

### Looking things up

- `search_web` searches the web, through Exa's free search (DuckDuckGo when Exa
  can't answer), and GitHub's repositories for code.
- `read_web` reads a page, a PDF, a text or code file, a GitHub repository or a
  file in one, a Max patch or Max for Live device (its controls and its gen~
  code first), or a picture. Asked for "an audio effect that sounds like the
  Erbe-Verb" and nothing more, Kumi reads how the original works before it
  writes any code.
- Kumi reads only public addresses (checked again as each connection is made,
  redirects too), and what a page says is information to it, never
  instructions. What it looked up shows above its answer, a line each.

### Fixes

- A rack Kumi just loaded takes chains at once: a change whose tool isn't
  listed yet reads Live's catalog again before saying it isn't available.
- Renaming a device added since Kumi connected works (bridge 1.0.53).
- A `/goal` with nothing to compare against ends and says why, instead of
  "setting up" forever.
- Renders record from the right place on real Live. Kumi no longer moves the
  playhead while Live is stopped (Live's "continue" plays from where it last
  stopped): with room before the part it jumps there while playing, and near
  the Set's start it records from the start.
- A command typed while a list is open runs, so `/update` works from the model
  list Kumi opens on its first start.

## 1.0.0 — 2026-09-30

The first release for producers to use day to day. Ships with bridge 1.0.52.

### Changes to the Set

- Kumi changes almost everything Live's scripting lets a script change. It
  started with tempo, the mixer, names, tracks and scenes, MIDI clips, Browser
  devices, parameters, locators and colours. Now it also handles:
  - the transport (loop, metronome, punch, playhead);
  - song settings (time signature, swing, quantization), Scale Mode and groove;
  - routing, arming and monitoring, extra mixer options and sidechains;
  - clip settings, audio clips (gain, pitch, warping, fades) and warp markers;
  - copying and moving clips, and Arrangement clips;
  - editing notes by id, quantizing, and 23 MIDI transforms and generators;
  - clip automation;
  - return tracks and duplicates, scene settings and scene capture;
  - devices on or off, moved (also across tracks and into racks) and deleted;
  - rack chains and variations, a Simpler's sample, audio import, Capture MIDI,
    device-specific settings and the Looper.
- Before a plan of three steps or more, or one that deletes, Kumi keeps a copy
  of the Set as last saved next to it, once for each saved version, and says
  where it is.
- Each change is in HISTORY with its undo. The few Live can't take back
  (deleting a device or return track, cropping a clip) are kept there with that
  said.
- Tools that needed bridge fixes aren't offered by an older bridge, and say to
  update it when a plan names one.

### Playing, recording and bouncing

- `play`, `fire_scene`, `launch_clip`, `record`, `jump_to_locator`, `select` and
  `show`, used only when you ask to hear, record or see something. NOW shows
  each as it happens.
- `wait` steps in a plan let a recording run. Resampling is one plan: an audio
  track fed from the source or "Resampling", armed, recorded in the Arrangement
  for the length you want, then disarmed.
- A plan that started playback or recording and then stopped short stops them
  again. `/stop` stops Live at any time. When Live refuses the ordinary stop,
  Kumi uses the bridge's emergency stop.
- Before recording, Kumi disarms any other armed track (Live 12.4 arms a MIDI
  track when it's made, and the bridge records onto one armed track only), each
  a change with its undo. A Session clip is launched, then recorded at once.
  `play back-to-arrangement` (bridge 1.0.35) presses Back to Arrangement, so a
  track that followed its Session clips plays the Arrangement again.
- The bridge says why it refused ("recording start requires the exact
  destination to be the only armed track") instead of "adapter request failed".
- A playhead, loop or locator past the end of the Set says where the Set ends
  and that nothing changed (bridge 1.0.36), instead of a change Kumi couldn't
  confirm.
- A track whose input became No Input (its source track was deleted) no longer
  counts as armed: Live can't disarm it and it records nothing, but it used to
  stop recording elsewhere (bridge 1.0.37).
- A loop or playhead change undoes after playing or recording since; the
  playhead goes back only while stopped. A routing change on a track that had
  No Input undoes too (bridge 1.0.38).
- When Live no longer offers what a track was routed from (an input with no
  audio device, a track that no longer makes sound), its undo is refused with
  that reason and nothing changed, instead of an undo Kumi couldn't confirm
  (bridge 1.0.39).
- Firing a scene is confirmed: Live 12.4 launches it on its next tick, and the
  bridge used to refuse it as unconfirmed right after the call (bridge 1.0.40).
  An empty scene, which would only stop what's playing, says it has no clips
  to play and launches nothing (bridge 1.0.41).
- Each device row says whether it's an instrument, an audio effect or a MIDI
  effect, and Live's selected device is named exactly, so FOCUS draws its
  icons and marks the right one of two same-named devices (bridge 1.0.45).
  Selecting a device in Live from Kumi was tried and left out: on real Live it
  lagged or landed elsewhere, so pointing at one stays inside Kumi.
- A track Kumi made as scratch (a render it recorded) is deleted on undo though
  its clip changed since, and an undo refused because something Kumi made
  was changed says nothing changed, rather than leaving the change uncertain
  (bridge 1.0.49; 1.0.46 checked it in the host, and Live refused it).
- One recording can take several armed tracks when Kumi names them all, so
  several sources render in one pass (bridge 1.0.48).
- Long sessions no longer run out of room: the bridge kept every applied change
  for its undo, 64 of a kind, then refused new ones (a match run's renders
  filled it in minutes). It keeps 512, and Kumi gives up the undo of its own
  render steps as it goes (bridge 1.0.50).
- Kumi's instructions say where note ids come from (the clip's notes, listed
  like any other part of the Set).

### Listening

- `audition` hears what Kumi built, quietly: it renders up to eight candidate
  tracks in one real-time pass (each track's Post FX onto a scratch track, with
  Main silenced), scores each against a reference from 0 to 100 on balance,
  brightness, movement, envelope, pitch, density and width, and names the
  biggest gaps. Then it removes the scratch tracks and puts everything back.
  Main is restored exactly after an error, a cancel or a crash (on the next
  start, with a word to the producer). A silent render is said as such, never
  compared. HISTORY shows one quiet line with the score, and the conversation
  one line per round ("Round 2 · 58% → 71% · …"). Needs bridge 1.0.49.
- A matching request ("make it sound like this") is a match run: when the
  model ends its answer, Kumi auditions where it got to and, short of the
  target, sends it back in with the score, the budget left and the biggest
  gaps. The run ends at the target, on a plateau (only after something
  genuinely different was tried), when its generous budget is spent, or on
  Esc; "keep going" carries it on. NOW shows the score and time as it works.
- `/goal` goes after a sound until Kumi gets there: the model sets up a few
  genuinely different candidates, then Kumi's own search nudges, crosses and
  redraws their knobs a generation at a time, every candidate rendered in one
  silent pass, with the model making structural leaps every few generations or
  when the search stalls. The GOAL tab shows it as it works: generations,
  candidates heard, the best with its trend, the leader, what was tried last,
  the time, and tokens on an API key. It ends at the target, at a safety cap of
  hours, or with /goal stop; Esc pauses it, and /goal picks it up again, after a
  restart too. The best so far lands on a "Kumi · Goal best" track, and every
  candidate's chain ends in a limiter. Needs bridge 1.0.50.
- Goals try nearly twice as many candidates a minute on real Live (5.9 to
  10.8 with four candidates): a goal's render rig keeps Main down (said once),
  the transport primed and recording on between passes, so a pass is play,
  wait, stop (about 12 s instead of 30). A long part (6 s or more) is screened on
  its most characteristic few seconds, with the best heard at full length every
  fourth generation, and settings already heard aren't heard again.
- A gap no knob closes (a band 9 dB or more off, an attack three times off,
  the wrong register, far too wide) that costs the most is named with the
  structural change for it: a missing sub asks for a sub layer, an EQ low
  shelf or another base. A match run's next round and a goal's next leap lead
  with it at once, instead of turning more knobs.
- Kumi learns from its match runs: each leaves a short lesson with its
  evidence ("plucked metallic percussion: Collision + parallel delays won,
  52% → 74%; Operator FM 52% → Collision 64% → …"), which the next matching
  run reads first. The producer's reaction after is noted with it. Lessons are
  Kumi's own, kept apart from the producer's techniques, and /memory lists
  them with forget.
- Matching a sound is a search: Kumi builds several different candidates,
  auditions them together, refines the best on its biggest gaps, and says the
  score before and after. A technique drafted while matching is kept only once
  it's been heard.
- `listen` hears audio files and the Set's audio clips. It measures loudness
  (LUFS, true peak, range), balance in ten bands with stereo width, dynamics,
  tempo and key. For a single sound it also finds pitch, harmonics, envelope
  and LFO movement.
- It compares audio with a reference at matched loudness, which helps with
  matching a mix or rebuilding a sound. The conversation shows a small spectrum
  and the differences in dB. The analysis runs on your computer; only numbers
  reach the model.

### Watching video tutorials

- `watch_video` watches a video you point Kumi to, a YouTube tutorial (or
  another site's) or a video file, so it can build what the video shows. It
  reads the title, chapters and words as timed lines. The words come from the
  captions or, without them, from the speech, transcribed on your computer with
  whisper.cpp. Kumi looks at frames where the narration names a device, a
  setting or a value, and looks again close up to read a value on screen. It can
  keep a stretch of the video's sound to compare with its own version.
- The conversation shows each video watched, with small pictures of the frames
  and their times; NOW says what Kumi is doing meanwhile.
- yt-dlp and the speech model are fetched when first needed, each checked
  against its published checksum, and so are ffmpeg (for frames, and for audio
  formats) and whisper.cpp on Windows and Linux; on a Mac they're yours
  (Homebrew), and `kumi doctor` says whether you have them. Videos are kept in
  `~/.kumi/videos`, so watching one again is quick.
- Kumi's agent core shows the model images from tools for the rest of that
  answer, then puts them away, keeping what was said around each.

### Max for Live devices

- `make_device` makes a MIDI effect you describe in your own words and puts it
  in your User Library's Kumi folder, for `load_device`. The model writes the
  device's name, knobs (ordinary Live parameters), code and tests; Kumi builds
  the device around them from a fixed frame. The frame parses MIDI, keeps count
  of held notes and runs timers, and hides files, the network and the rest of
  Max and Live from the code.
- Before it's made, Kumi runs the code on your computer against its tests and
  Kumi's own checks (no errors, no hanging notes, nothing running once every
  note is released), and refuses a device that fails, saying why so the model
  can fix it.
- The model reads the guide (what the code can use, the rules and the craft)
  only when it makes a device, so ordinary requests don't carry it.

### Recipes

- Kumi saves ways of working as recipes (`save_recipe`) and replays them
  (`run_recipe`, or `/recipes`), in any Set, with blanks filled when they run.
- `watch_me` learns a routine you do by hand. It compares the Set before and
  after, including the knobs you turned on devices you added, so Kumi can save
  it as a recipe.
- A plan or recipe names a device's parameters (`parameter: "Drive"` on the
  device an earlier step loaded), so saved routines keep their settings. A saved
  step can't hold a reference that only means something in the session it was
  saved in.

### Memory

- Kumi keeps short notes of what you tell it that Live can't show, about you and
  about each saved Set.
- Techniques: when Kumi builds a sound or a chain (from a tutorial, or a request
  of several steps), it drafts what made it work (the model's draft, or, when
  the model gives none, one from the build: what you asked for, the chain in
  order, the settings it changed), and keeps it only if your next
  moves say you liked the result (you played it, kept working on it, saved the
  Set, said so, or moved on and left it). It's dropped quietly if you undid it,
  deleted it or said no. Kept techniques are named in the model's instructions,
  read whole when a request fits, adapted, and Kumi says it's using one.
- Every save shows: a line of its own kind in the conversation (`✎` notes, `◆`
  techniques, `↻` recipes), a moment in NOW, and the latest three at the top of
  the HISTORY tab with a forget click on each. `/memory` lists and forgets all
  three kinds.
- The right pane: FOCUS and NOW on top, and in its bottom half a tabbed area,
  HISTORY its first tab, scrolled by the wheel or Shift+Tab and the arrows (Enter
  undoes a row). The tab showing is remembered. Later tabs slot in as modules.
- What Kumi couldn't do for lack of a tool or Live's scripting is logged on your
  computer for Kumi's developers (`~/.kumi/gaps.jsonl`), never read back and not
  part of what Kumi remembers.

### Conversations and reconnecting

- If Live closes, or Kumi's bridge to it drops, Kumi says so, keeps the
  conversation and reconnects on its own when Live is back (a late answer from
  the bridge to a stopped request used to drop the connection for good). A
  request Live's going away stopped is back in the input box, one Enter from sent
  again. After 30 seconds without Live, Kumi asks whether it's open with the
  bridge on. `/reconnect` tries again at once, and nothing about a connection
  problem suggests `/new` any more. The first answer after a reconnect is told
  that references from before are gone, so it doesn't try them first.
- Each Set's conversations are kept, the latest 20, an unsaved Set's too (they
  move with the Set when it's first saved), with the HISTORY of each. `/new`
  forgets this conversation and starts fresh, keeping the old one on screen
  under a line and in `/conversations`, which goes back to any of them. `/new`
  keeps the bridge, so earlier changes can still be undone.
- ↑ and ↓ go through what you sent before, kept across `/new` and restarts in
  `~/.kumi/input-history`, with keys and tokens left out.

### Models and sign-in

- `/model`, `/effort`, `/login` and `/logout` inside Kumi. Models are listed
  from each provider, so new ones appear without an update.
- Signing in covers ChatGPT plans and Anthropic, OpenAI or OpenCode API keys. A
  failed sign-in offers the fix and sends your message again.

### Speed

- Tool calls start while the model is still writing them. A plan's steps run as
  they arrive, and a finished plan needs no second model reply.
- The observation before each answer is one round trip to Live.
  `npm test` holds these to counted latency budgets.

### Setup

- On a nearly full disk, recording, making a device and fetching ffmpeg or a
  speech model are refused first, saying what's free and what to do, instead of
  failing partway.
- On an API key, `/status` says the tokens this session's answers took (in,
  cached, out).

- `npm run kumi -- bridge` installs the bridge into Live, or updates it, in one
  command. It runs the bridge's own lifecycle (plan, apply, rollback), refuses
  while Live is open, and waits for Live afterwards. `kumi doctor` and a failed
  start say when it's needed.
- `kumi update` brings the checkout up to date, rebuilds, and updates the
  bridge in Live when it's older. Kumi says when a newer version is out (asked
  of git once a day at most) and, as it starts, when Live's bridge is older.
- `kumi report` writes one file to send when something goes wrong: versions,
  the doctor, the last conversation's requests and tool calls, the gap log and
  the bridge's lines from Live's log, with keys, tokens, the home folder and the
  account name taken out.

### Also

- `kumi --version`. Kumi identifies itself as `kumi/1.0.0` to providers and the
  bridge.
