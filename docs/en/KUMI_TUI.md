# Kumi commands, keys and screens

English · [简体中文](../zh-CN/KUMI_TUI.md) · [日本語](../ja/KUMI_TUI.md)

The reference for Kumi's terminal app: what's on screen, every command and key,
and the plain mode. The [guide](KUMI_GUIDE.md) explains what Kumi does.

## The screen

Kumi takes the whole terminal window and gives it back as it was when Kumi
closes, crashes or is stopped.

- **Header:** Kumi, the Set's name, the model and its effort, the background
  jobs at work (◐ 2 jobs), and Live's connection. While Live plays, a yellow light blinks on its beat (brightest on
  the bar's first beat) beside the tempo. While your fallback model answers,
  it's named there with "· fallback"; near a plan's limit (80% of a window
  used), "plan 84% · resets 14:20" shows beside the model. Once the model's
  context is more than half full, a meter beside the model says how full
  ("context ▰▰▰▰▰▰▱▱▱▱ 62%", in the warning colour from 80%); `"contextMeter":
  "always"` in settings.json shows it from the start, and `"off"` never.
- **Conversation** (left): your messages, Kumi's answers and its steps, in plain
  words ("looked at your Set"), each with its timing. A step at work animates by
  kind (searching, reading, building a device, changing, listening, playing);
  the same step done several times folds into one line ("read a page ×3").
  What Kumi heard shows as a small spectrum, and what it keeps as a line of its
  own: ✎ a note, ◆ a technique, ↻ a recipe, ✦ a lesson from matching. While the
  model thinks between steps, the headline of its reasoning shows on a row of its
  own, as a step at work. Each finished answer ends with one quiet line: its
  time, its changes, and its tokens on an API key or its share of the plan on a
  subscription ("6.2s · 2 changes · 18.4k in · 1.2k out").
- **FOCUS** (top right): where you are in Live, following your selection as it
  changes: a track's device tree with rack chains, a clip's notes as a small
  piano roll, or a Session or Arrangement strip. A device you point at (click
  it, or use **Ask Kumi about this** in Live) is pinned for your next messages.
- **NOW** (middle right): what Kumi is doing, drawn as it happens: a value's
  before and after, an EQ's or a compressor's curve, an automation lane, a new
  clip's notes, a colour swatch, a device landing in its chain, "▶ Playing from
  the start marker". While Kumi waits for the model, it
  says why and until when: "retrying in 5s · ChatGPT is busy", "carrying on at
  14:20 (in 2h 13m) · ChatGPT's plan limit", "offline · trying again in 20s"
  (Esc stops any of them). While the model thinks, NOW says for how long
  ("thinking · 31s"), with the headline of its reasoning: one line, never the
  reasoning itself, which stays out of the answer and of `/copy`.
- **Tabs** (bottom right): **HISTORY** lists the latest three things Kumi kept
  (each with **forget**), then every change, newest first, each with **undo**,
  or **kept** / **no undo** / **check Live** when it can't be undone. A change
  opens to its picture: an EQ's curve, a compressor's, the notes (new ones in
  colour, gone ones faint), an automation lane, or each value before and after,
  with what was asked when Live set something else ("asked -2 dB"). **GOAL**
  shows the `/goal` ("No goal yet" until there is one): the goal, its turns done
  and time against the budget (the status line counts the turn under way), the
  last check and what's next; for a sound-match
  `/loop`, the best score with its trend, the leading candidate, and the time.
  **JOBS** joins them with the first background job: each one's progress, time
  and tokens, newest first, with **stop**; Enter opens its steps and report.
- **Input box** (bottom left): messages waiting to be sent show above it, a
  pinned device as a chip, and the files that go with your next message (name,
  kind, size and ×). Empty, it says "ctrl+t to talk". While Kumi listens,
  its bottom line shows a pulsing mint `●`, the time and a level meter, with the
  keys that apply at its right; no red, which in Live means recording.

The welcome screen shows what changed in the Set since you were last here, a
newer Kumi when there is one, and, the first time, that Kumi is learning your
library in the background.

The first time Kumi opens after an update, the conversation starts with **What's
new**: up to five of the newest notes from Kumi's changelog, and `/changelog` for
the rest. A conversation carried on from last time goes above them. A fresh
install shows none, and the model never sees them. `"whatsNew": false` in
`~/.kumi/settings.json` turns them off.

A conversation carried on comes back as you left it: its last 30 requests with
their steps, pictures and listening rounds, and above them how many earlier
messages aren't shown. HISTORY shows its changes, which Kumi can't undo after a
restart (your DAW's own undo still can). A conversation kept by an older Kumi
comes back as its words, as before.

**Setup** comes first when a step is missing: signing in, Kumi's bridge in Live
(missing, or older than Kumi's), and then the Control Surface. It lists the three
steps, done ones in mint with what was chosen, and asks what the current one
needs; a mint waveform under the wordmark moves while Kumi waits (for the
browser, the install, or Live). ↑↓ and Enter choose, Esc leaves the step for
later, and the session follows in the same window. While ChatGPT's sign-in waits,
**c** copies its link (or code), **d** uses a code instead of the browser, and **b**
the browser again. On
Linux, setup only signs in. A Kumi that's set up starts straight into the session.

Below 100 columns (a window beside your DAW) the pane is a **drawer** under the
conversation: a row of tabs (NOW, FOCUS, HISTORY and the others) over the tab it
shows, 6 to 10 rows by the window's height.

- A change Kumi makes opens it to NOW: what Kumi is doing, the change with its
  undo, and its picture. A goal opens it, folded, on GOAL. What Kumi opened folds
  back to its row of tabs after 10 seconds with nothing new.
- Click a tab to open it there, or the tab showing to close it; Shift-Tab and
  Tab open it on the tabs and on FOCUS's tree as they go into them. What you open
  stays open, on the tab you chose, until you close it. It never takes the
  keyboard from the input box.
- Ctrl-\\ opens and closes it. Once you close it, Kumi leaves it closed until
  you open it again.

Below 24 rows the pane is a two-line strip above the input box instead (where
you are and what Kumi is doing, then the last change with its undo). Below 24×8
Kumi asks for a bigger window.

## REAPER and Renoise

The screen is the same in REAPER and Renoise
([connecting](KUMI_GUIDE.md#connect-to-reaper-or-renoise)), in the DAW's own
words and views.

- **Header:** the project's or song's name and the DAW's connection
  ("● REAPER", "REAPER busy" while a dialog or a render holds it). The beat
  light follows the DAW's own time and shows its meter beside the tempo:
  REAPER's time signature through its tempo map ("7/8"), Renoise's lines per
  beat ("LPB 4").
- **FOCUS in REAPER** follows what has REAPER's focus:
  - the **arrange view**: the stretch it shows, scrolled and zoomed as in
    REAPER ("bars 9–16 of 64"), and the tracks around the selected one, indented
    by folder (a collapsed folder hides its tracks); their items as bars, a row
    for each fixed lane (a lane that doesn't play is faint), markers, regions
    from start to end with their names, the loop and the playhead;
  - the **MIDI editor**: the take's notes as a small piano roll, with up to three
    lanes under it (CC, pitch, program, channel pressure, poly aftertouch) and
    "and n more lanes";
  - an **FX chain**: the chain as a tree, input FX as a branch of their own,
    containers open with their FX, and bypassed, offline and parallel FX marked;
    a take's chain when its FX has the focus ("Kick · take “kick.wav”");
  - a track's **routing** window: its sends, receives and hardware outputs with
    their levels, and its MIDI hardware output;
  - the **mixer** window, undocked (a docked mixer can't be told apart): the
    master and the tracks around the selected one, with volume, pan, mute, solo,
    arm and a peak meter.
- **FOCUS in Renoise** follows its middle frame and panels (Renoise doesn't say
  which has the keyboard, so FOCUS goes by what changes):
  - the **Pattern Editor**: the lines around the cursor (or the playhead,
    following playback while Follow Player is on), each track's note,
    instrument, volume, panning, delay and effect columns in Renoise's notation,
    track names in their colours;
  - the **Pattern Matrix**, while it's shown: the sequence's slots with their
    patterns, sections and muted tracks, and the current slot;
  - the **Phrase Editor**: the same grid, with the phrase's LPB, key tracking and
    looping;
  - an **instrument** (the Instrument or Sample Editor): its keyzones as a map of
    keys, each sample with its mute group and slices, its macros, phrases,
    modulation sets and FX chains;
  - **Track DSPs**, while that panel is open: the chain as a tree, a Doofer with
    its devices, a Splitter with a branch per lane (Frequency Low and High, Mid
    and Side, Parallel Lane 1 and 2), bypassed devices marked;
  - the **Mixer**: each track's post-FX volume, panning, mute and solo (no
    meters: Renoise's API has none).
- **Pointing:** Tab and clicks work in the FX and DSP trees as in Live's device
  tree.
- **Pictures** in NOW and HISTORY, one per kind of change: a pattern's or
  phrase's lines before and after, an envelope's or an automation's points, the
  parameters that moved (named, with bars), a route's values (sends, receives,
  hardware outputs), keyzones, items on their track, settings before and after,
  and a track's devices after a chain loads. Keyzones and items are drawn (as
  images where the terminal shows them); lines and routes are words.
- **Setup** has two steps there, on Linux too: Sign in, and Connect to REAPER
  (or Renoise). There's no Control Surface.
- **Commands** speak the DAW's words: `/refresh` reads your REAPER project again,
  and `/stop` stops REAPER's transport and recording, and what Kumi plays out
  loud. `/willington` is Live's.
- **Away from Kumi:** Kumi's line goes to Renoise's status bar. REAPER has no
  status bar Kumi can write to, so a notification tells you; `/status` says
  which.

## Commands in a terminal

Run these as `kumi <command>`; from a copy of the repository, as
`npm run kumi -- <command>`.

| Command | What it does |
| --- | --- |
| `kumi` | Open Kumi on what's open in your DAW (Live, REAPER or Renoise, opened before or after Kumi); the first time, sign in and put Kumi's bridge in that DAW (Kumi can restart Live for you). Open another DAW while Kumi isn't connected, and Kumi follows |
| `kumi --daw live`, `kumi --daw reaper`, `kumi --daw renoise` | Work with that DAW only, whichever DAW is open; the first time, sign in and put Kumi's bridge in. `KUMI_DAW` does the same; `kumi --daw reaper doctor` (or `report`) speaks of that DAW |
| `kumi --daw reaper bridge [--yes]` (or `renoise`) | Put Kumi's extension into REAPER, or its tool into Renoise, or bring it up to date; `--yes` answers its question beforehand |
| `kumi -c` (or `--continue`) | Carry on the conversation you had last, in any project |
| `kumi -r [id or name]` (or `--resume`) | Carry on a kept conversation by its id (or its start) or name; alone, choose one. Both work with `--inference-only` |
| `kumi export [id or name \| --last] [--md \| --html \| --json] [--full] [--raw] [-o path]` | Save a kept conversation as Markdown (the default), a web page or a file for Kumi; without an id (or with `--last`), your latest. `--full` adds each step's input and result; `--raw` leaves keys and paths in |
| `kumi import <file>` | Keep a conversation saved with `kumi export --json`, to carry on with `kumi -r` |
| `kumi --inference-only` | Chat without a DAW |
| `kumi -p "<request>" [--json]` (or `--print`) | Run one request, print its answer and end, for scripts. Anything that went wrong goes to stderr, and the exit code says how it ended: 0 answered, 1 failed, 3 stopped at its step limit, 130 stopped with ctrl+c. `--json` prints each event Kumi sends as a JSON line instead. A fixed phrase ("tempo 124") changes the DAW at once, with no model. With a DAW, the request goes into the open project's conversation, as in `kumi`. Works with `--inference-only`, `--daw`, `-c` and `-r <id or name>` |
| `kumi --bridge-config <absolute path>` | Use your own bridge configuration |
| `kumi login` | Sign in: asks whether with ChatGPT or an API key |
| `kumi login <provider>` | Sign in to `openai-codex` (ChatGPT; `--device` without a browser, the default over SSH; `--browser` for the browser), or `anthropic`, `openai`, `opencode` or `opencode-go` with an API key |
| `kumi logout <provider>` | Remove Kumi's sign-in there |
| `kumi model [<provider>/<model>]` | Show or choose the model, a model on your computer too (`ollama/<model>`) |
| `kumi auth` | Which providers are usable, the model and the sign-in file (never secrets) |
| `kumi bridge [--yes] [--allow-dirty]` | With Live closed: put the bridge into Live, or bring it up to date. `--yes` confirms Live is closed (without a terminal, nothing changes without it); `--allow-dirty` lets a checkout with uncommitted changes install its bridge |
| `kumi doctor [--json]` | Check the installation (version, rollback, PATH, updates, free space, Live, its User Library, the bridge's ports, Developer Mode), sign-in, the model servers on your computer, the bridge, Live, the extension, your library, video programs, talking, Live's menus, the terminal, and REAPER and Renoise where you use them; says what to run. `--json` prints it without keys or your home folder |
| `kumi library [--rebuild]` | How far Kumi has got learning your sounds, presets and Sets; `--rebuild` learns them all again |
| `kumi update [--check \| --rollback \| --version <x.y.z>]` | Get the newest Kumi (and the bridge when it's older, and the copies of REAPER's extension and Renoise's tool it carries); `--check` only asks; `--rollback` goes back to the one before, with its bridge when Live is closed; `--version` installs that release, newer or older (an installed Kumi only) |
| `kumi report` | Write `~/kumi-report-<date and time>.txt` to send when something goes wrong |
| `kumi uninstall [--all] [--yes] [--dry-run]` | Remove an installed Kumi and what it fetched; `--all` also removes conversations, notes, recipes, sign-ins, library and references; `--yes` skips the first question; `--dry-run` says what would go |
| `kumi --version` (or `-v`), `kumi --help` (or `-h`, `help`) | Version; help |

## Commands inside Kumi

Type `/` for a menu of these: ↑↓ choose, Tab completes, Enter runs, Esc closes.
After a command and a space, the menu offers its words: `/palette use` your
palettes, `/resume` your kept conversations, `/slots listening` the models the
slot takes, `/model` the models you can use, `/export` its formats. Tab puts the
one chosen in; Enter runs only what you named, so a word only partly typed goes
in first.
A message that starts with a path (a file dragged into the terminal) is a
message, not a command.

| Command | What it does |
| --- | --- |
| `/new [name]` | Start a fresh conversation, named if you give one. What's above stays on screen under a line, the last conversation is kept, and HISTORY's undo still works |
| `/btw <question>` | Ask something on the side, any time, even while Kumi works: answered from the conversation so far, without tools, in a panel, and not added to the conversation. `/btw` alone shows the last answer again |
| `/resume [id or name]` | Kept conversations: this Set's, and every Set's under Tab; choose one to carry on with it. With an id (or its start) or a name, that one. `/conversations` is the same |
| `/rename [name]` | Name this conversation; alone, its name goes in the box to change |
| `/fork [name]` | Carry on in a copy of this conversation, with a new id; the original stays |
| `/export [copy \| md \| html \| json]` | This conversation as a file: copy it as Markdown, or save Markdown, a web page or a file for Kumi (`kumi import`); alone, choose. Over 64 KB, the copy is saved as a file instead |
| `/share` | Save this conversation as a web page that opens anywhere (the same page again while nothing changed), then open it, show it in Finder (Explorer) or copy its path, until Done; over SSH, it says where the page is |
| `/reconnect` | Connect to Live again over a fresh bridge, keeping the conversation (earlier changes lose Kumi's undo) |
| `/daw [name]` | Which DAW Kumi works with; `/daw live`, `/daw reaper` or `/daw renoise` closes Kumi and opens it again with that one. Shown once you work in REAPER or Renoise, or have Kumi's bridge in one |
| `/undo` | Undo Kumi's latest change |
| `/takeback [n]` | Take back your last request (or your last n): Kumi undoes their changes in your DAW, newest first, through its own records, and the conversation goes back to just before them, with the request's words in the box to change and send again. A change from before Kumi last started stays as it is; if an undo fails (you changed it since, say), Kumi stops there and says which |
| `/stop` | Stop Live: clips, the transport and recording. Also stops Kumi's answer |
| `/refresh` | Read the Set again without asking the model |
| `/copy` | Copy Kumi's last answer to the clipboard |
| `/model`, `/effort` | Choose the model (from each provider's own list, then the model servers on your computer: Ollama, LM Studio and those in settings.json; type to filter) and how hard it thinks; from your next message. In `/model`, ctrl+f makes the selected model your fallback (again clears it). `/model <id>` chooses one by its id, which Tab completes |
| `/fast` | Turn on the model's faster tier when its provider lists one (ChatGPT's "Fast": quicker answers, more of your usage); `/fast` again turns it off. Shown as "· fast" beside the model |
| `/willington` | Turn [Willington](WILLINGTON_INTEGRATION.md) on or off. Rack bindings are requested on supported builds; clip Follow Actions and native editing also require passing exact-library self-tests. Native editing is b5 macOS ARM64 only. Off until enabled; available when the bridge carries Willington. |
| `/login`, `/logout` | Sign in (ChatGPT in the browser or with a code, or an API key shown only as dots) or out |
| `/loop <what to reach>` | Work in judged rounds: one change, heard before and after, kept only if its target improved and nothing else got worse, then the next target. It stops when every item is met, when changes stop helping, or at 16 rounds or 45 minutes; Esc or `/loop stop` stops it. A sound to match ("make my pad sound like ~/ref.wav") runs the knob search instead: `/loop` alone picks a paused one up, `/loop stop` ends it |
| `/goal <what to reach>` | Keep at one goal across turns, checked after each (by the judge's numbers when measured), until it's met, blocked, stuck (three turns in a row that got no closer) or out of budget. `/goal` shows it; `/goal resume` (a fresh budget for whatever ran out), `/goal edit <words>`, `/goal new: <words>` (replaces an unfinished one), `/goal pause` (or Esc), `/goal clear` (with no unfinished goal, a paused sound-match search) |
| `/slots` | Which model does each listening job (stems, transcription, listening, embeddings), and swaps one in plain words: `/slots listening gemini`, `/slots listening off`, `/slots listening <base URL>#<model>`, `/slots embeddings <.onnx file or https link>`. A new model is tried first; `/slots back <job>` takes a swap back |
| `/music` | Your own music, which Kumi learns your taste from on this computer: its folders, the songs heard and how it's going. `/music add <folder>`, `/music remove <folder>`, `/music pause`, `/music resume`, and `/music forget` (deletes what Kumi learned, after you confirm). Song and artist names reach the model when Kumi uses your taste |
| `/map` | Your sounds and songs as two maps, full screen (Tab switches): sounds coloured by class, songs by style. The arrows move to the nearest point that way, Shift and the arrows pan, + and − zoom, a click chooses a point, `/` filters by class, folder, pack or genre tag, Space plays the point, and a point plays as soon as the mouse is on it (two seconds of it; `h` turns that off and on, and Kumi remembers). Drag a box, Ctrl and a click, `x`, `a` (all the filter keeps) or `g` (each one's nearest) pick points, and Enter puts them (or the point chosen) into your palette; Esc lets them go first. The map turns so its long side runs across the screen. Zoomed out on a big library, the busiest places are brightest; as you zoom in, points grow (a typical sound a little bigger), a soft light shows where they gather, and names appear. In braille, zooming and moving glide. An image where the terminal shows images (kitty, Ghostty), braille elsewhere, a list on a basic console. Esc closes it |
| `/map open` | The map in your browser: a point's name when you point at it, the wheel zooms, Shift and drag (or Lasso) picks points, the filter keeps what has its words, groups show or hide, and a click plays a point. Its actions put their message in the input box, and Add to palette puts the points picked into your palette. Served from this computer only, at a link with a random key; over SSH, Kumi says which port to forward. `/map close` closes it, and so does quitting Kumi |
| `/palette` | Your palettes: the sounds you gather to work with, on `/map` or the browser's map. `/palette` lists them; `/palette new <name>`, `use <name>`, `rename <name>` and `forget <name>`. They're kept between sessions, and each session works with one. Kumi reads it with each request: `/palette strict` keeps Kumi to its sounds, and `/palette loose [k]` lets each sound bring its k nearest in your library (5 unless you say), the ones your own projects use first. Those are marked faintly on `/map`, and Kumi says when it uses one and offers to add it. PALETTE, in the right pane, lists its sounds: Space plays one, `x` takes it out, and its last row asks Kumi to load them as a kit |
| `/memory` | Everything Kumi keeps: notes about you and this Set, what it learned from your Sets, techniques, recipes and lessons; choose one to forget it (a note to change its words or pin it, a recipe to run or forget) |
| `/note <id> <new words>` | Change a note's words without the model; a note's "Change the words" in `/memory` starts it for you |
| `/recipes` | Your recipes: run one or forget it. One with blanks starts a `/recipe` line for you to finish, with what's pinned filled in |
| `/recipe <name> blank=value …` | Run a recipe now, with no model call; a value with spaces goes in quotes, an unquoted number or true/false goes as itself (quote it to keep it words), and Kumi names any blank left empty |
| `/jobs` | The background jobs: Enter opens one, where `x` stops it and `r` runs an interrupted one again. Works while Kumi answers |
| `/status` | What Kumi is connected to, the model, how far it has got learning your library, how many settings its searches heard and remember (and on how many devices) and how often one remembered won the round it was tried in, of the searches that started from one, on an API key the tokens this session used, and the conversation with its id |
| `/voice` | Talking instead of typing: start or stop, "Send when you stop", the language you speak and the microphone |
| `/update` | Get the newest Kumi: it asks, closes, updates and opens again with the same conversation |
| `/changelog` | What's new in Kumi: every note since the version you updated from, or the latest three releases, and where the whole history is |
| `/help` | The keys and commands, in sections, in a panel that scrolls; `?` on an empty box opens it too |
| `/quit` | Close Kumi |

`/new`, `/reconnect`, `/refresh`, `/undo` and `/takeback` work only while Kumi
isn't answering; during an answer, Kumi says so and leaves things as they are.

## Instant phrases

A message that's one of these phrases changes Live at once, with no model call.
Its answer says what changed and "instant", and the change is in HISTORY with
its undo. They're in English for now.

| Phrase | What it does |
| --- | --- |
| `tempo 124`, `bpm 124`, `124 bpm` | Sets the tempo |
| `mute drums`, `unmute drums` | Mutes or unmutes a track |
| `solo bass`, `unsolo bass` | Solos or unsolos a track |
| `arm vocals`, `disarm vocals` | Arms a track to record, or disarms it |
| `loop 9-16`, `loop bars 9 to 16`, `loop off` | Loops bars 9 to 16 (both included), or turns the loop off |
| `metronome on`, `metronome off` | Turns the metronome on or off |

- A track is its name (any case), the one track whose name starts with what you
  typed, or its place: `mute track 3`.
- Anything more ("mute the drums a bit", "solo the bass?"), or a track Kumi
  can't tell for sure, goes to the model as you wrote it.
- They run only while Kumi isn't answering; during an answer, Enter steers it as
  usual.
- **In REAPER:** a project with tempo or time signature markers sends `tempo`
  and `loop` to the model (which marker a tempo means, and how long a bar is,
  depend on where). `loop 9-16` also turns repeat on.
- **In Renoise:** `loop 9-16` loops the sequence slots that hold those bars; bars
  that start or end inside a pattern go to the model. `arm` is refused: Renoise
  records into the selected track's pattern while edit mode is on. The tempo
  goes from 32 to 999 BPM.
- `"instantPhrases": false` in `~/.kumi/settings.json` turns them off.

## Keys

**Typing and sending**

| Key | Action |
| --- | --- |
| Enter | Send. While Kumi works, it reads the message after the step under way; one that names something with `@` waits for the answer |
| Tab, while Kumi works | Send the message after the answer instead; it waits above the box |
| Alt-↑ | Take the last waiting message back into the box |
| Ctrl-J, Alt-Enter, Shift-Enter | New line (Shift-Enter in terminals that report it) |
| ↑ and ↓ | Move through the box's lines, then through what you sent before (kept across `/new` and restarts, without secrets) |
| Ctrl-A / Ctrl-E, Home / End | Start / end of the line |
| Alt-← / Alt-→, Ctrl-← / Ctrl-→, Alt-B / Alt-F | Word left / right |
| Ctrl-W, Alt-Backspace, Ctrl-Backspace | Cut the word before the cursor |
| Ctrl-K / Ctrl-U | Cut to the end / start of the line |
| Ctrl-Y | Put back what you cut last (cuts in a row, like Ctrl-W twice, come back together); Alt-Y right after swaps it for the cut before |
| Ctrl-- (Ctrl-_) | Undo in the box: a run of typing or deleting, a paste, a cut or what you dictated at a time. Where Ctrl-- zooms the terminal out (Windows Terminal, GNOME Terminal, Konsole), use Ctrl-_, that is Ctrl-Shift-- |
| Ctrl-R | Look back through what you sent for the words you type: Ctrl-R again finds an earlier one, Enter puts it in the box, Esc goes back to your draft |
| Ctrl-G | Open the draft in your own editor, the one VISUAL or EDITOR names (one that opens a window has to wait for it: `code --wait`). What you save comes back into the box, and Ctrl-- takes it back |
| Ctrl-S | Put the draft aside, with its files; Ctrl-S in an empty box brings it back (with words in the box, they change places) |
| Ctrl-T | Talk instead of typing: press it again to stop, or hold it while you talk. What you said lands in the box at the cursor; Enter stops and sends at once, Esc drops it |
| Ctrl-V | Add the picture on the clipboard (a screenshot, say) to your next message; files dragged into the window are added the same way |
| `@` | Name something exactly: a track, a device, an instrument, or a sound, palette or technique of yours. What you type after it finds them; Tab or Enter puts in the one chosen (`@Drums`), and Kumi is told exactly that one. Backspace takes it out whole; ↑ and Ctrl-G give it back as words, to name again |
| A long paste | More than 10 lines or 1,000 characters goes in as one chip, `[pasted 41 lines]`: Backspace takes it out whole, Kumi gets all of it, and the conversation shows the chip and the start of its first line. A message held while Kumi works, taken back with Alt-↑, put back after a stop, or brought back with ↑ or Ctrl-R keeps its chips. One message takes up to 256 KB: a longer one says so under the box, and Enter keeps it there. The history file keeps a long message's first 4,096 characters, ending in `…`; ↑ brings back one sent since Kumi started whole |
| Backspace, in an empty box | Take back the last file added |
| `?`, in an empty box | The keys and commands, as `/help` shows them; anywhere else, a question mark. To start a message with one, type a space first, or paste it |

**Stopping and moving around**

| Key | Action |
| --- | --- |
| Esc | Close the `/` or `@` menu; otherwise stop Kumi's answer (the steps it finished stay, waiting messages go back into the box); otherwise unpin the pinned device |
| Esc Esc | In an empty box while Kumi is idle, with nothing pinned: your requests, newest first, each with how many changes it made. Choose one to take it back with each one after it, as `/takeback` does (first Kumi lists the changes it can't undo, which stay as they are), or to carry on from before it in a copy, which leaves your DAW and the conversation as they are |
| Ctrl-C | Stop Kumi's answer; when idle, clear the box (↑ or Ctrl-- brings the words back); with an empty box, quit |
| Ctrl-B | Move the answer's slow step that doesn't use Live (a video, pages, the manual, your library) to the background as a job; the answer carries on. In tmux, press Ctrl-B twice |
| Ctrl-D | Quit, when idle with an empty box |
| Page Up / Page Down, mouse wheel | Scroll the conversation (the wheel over the tabs scrolls the tab); it stays put while new text arrives. Scrolled up, the top row keeps the request whose answer you're reading (a click goes back to it), and the bottom row counts what came in below |
| Ctrl-Home / Ctrl-End | The start of the conversation / back to the latest |
| Ctrl-O | The conversation in detail: each request, what each step was given and gave back (as the full export shows them, steps cut to 12 lines), each answer (with the model that gave it, when it started and what it cost, for those given while this Kumi has run), and the turn under way. In it, `/` searches (Enter keeps the search, Esc drops it), `n` and `N` go from match to match, `{` and `}` from request to request, `[` prints it into the terminal's scrollback until a key, `v` opens it in your editor (`VISUAL` or `EDITOR`), and `q`, Esc or Ctrl-O closes it |
| Ctrl-L | Redraw the screen |

**The Live pane**

| Key | Action |
| --- | --- |
| Ctrl-\\ | Below 100 columns, open or close the drawer; at full width, put the right pane away (the conversation takes the whole window, and a line under the box says how it comes back) or bring it back; below 24 rows, the same for the two-line strip |
| Tab, when idle | Into FOCUS's device tree, at Live's selection: ↑↓ move, Enter points at the row, Esc or Tab goes back to typing |
| Shift-Tab | Into the tabs (again for the next tab): ↑↓ and Page Up/Down move. On a change, Enter, → or Space opens its picture, Enter or ← closes it, and `u` undoes it while it's open; on a kept item, Enter forgets it; on a JOBS row, Enter opens the job and `x` stops it. Esc or Tab goes back |
| Mouse | Click **undo** or **forget** at a row's end, a change's words to open its picture (a job's to open the job), a tab's name, or a device in the tree to point at it; click the pin to unpin. Drag across text to copy it as you let go; double-click copies a word and triple-click a line (lines Kumi wrapped come out whole). The view holds still while you drag, and "Copied" shows for a moment where the hints are. A Ctrl-C just then copies again rather than stopping Kumi or quitting, and an Esc while Kumi is idle only takes that line away. With `"copyOnSelect": false` in settings.json, the text stays selected until Ctrl-C or right-click copies it, and Esc clears it. Shift-drag (Option in iTerm2) uses the terminal's own selection, for when Kumi's copy can't reach your clipboard (over SSH, where the terminal doesn't allow it) |

**Panels** (`/model`, `/effort`, `/login`, `/memory` and the others): ↑↓ move,
Enter chooses, Esc closes; in `/model`, `/memory`, `/resume` and
`/recipes`, typing filters the list. In `/model`, ctrl+f sets the fallback.
In `/resume`, Tab switches between This Set and All Sets, and ctrl+f renames,
pins, deletes or copies the id of the selected conversation. When a
conversation belongs to another Set, Kumi asks: open its Set in Live
(recommended), carry it on here as a copy, or cancel (Esc).
**Your answer** (an answer that ends on a question with numbered options): a
number, then Enter, answers. An option Kumi recommends is marked and chosen to
start with; when nobody's there (a `/goal`, `/loop` or match ran, or Kumi's
window has been behind another for 2 minutes), Kumi takes it after a countdown
("Taking 2 (recommended) in 60 s · any key cancels"). In the key box, paste the key and press
Enter; Kumi checks it with the provider, or saves it unchecked when the provider
can't be reached. In the ChatGPT sign-in panel, `c` copies the link (or the code), `d` signs in
with a code instead of the browser, and `b` goes back to the browser. In the
`/btw` panel, ↑↓, Page Up/Down and the mouse wheel scroll, ←→ go through
earlier answers, `c` copies, and Esc, Enter or Space closes it. In a job's
details, ↑↓ and the wheel scroll, `x` stops it, `r` runs it again, `c` copies
its report, and Esc closes them. In the help, ↑↓, Page Up/Down, Home, End and
the wheel scroll, and Esc, Enter or `?` closes it; typing or pasting closes it
too, and what you type goes into the box. With `"mouse": "off"`, the wheel
scrolls none of these.

**Your own keys.** `~/.kumi/keys.json` gives Kumi's actions other keys (`KUMI_KEYS_FILE` says another place for
it):

```json
{ "composer.undo": ["ctrl+z"], "app.background": ["ctrl+b", "f5"], "pane.toggle": [] }
```

- An action's list replaces its keys, and an empty list takes them away. A key is `ctrl`, `alt` (or `option`) and
  `shift` with a name: `enter`, `tab`, `esc`, `backspace`, `delete`, `insert`, `up` or `↑` (and the other arrows),
  `home`, `end`, `pageup`, `pagedown`, `space`, `f1` to `f12`, or one character (`ctrl+-`, `ctrl++`). Two keys with a
  space between (`esc esc`) are pressed one after the other: the first starts the pair only when nothing else in the
  box takes it, the second comes within half a second, and pairs work only in the box.
- Where actions share a key, keys.json's come first, then Kumi's own, in the order they always were.
- A character can't be a key where words are typed (the box, a list, the `/` and `@` menus), nor for what only a
  search or a filter does while it's typed (the detailed view's, the map's): it types itself there. `?` on an empty
  box always opens the help, and `app.help` can have keys as well (`f1`).
- Kumi says once, as it starts, what it couldn't use: an action it doesn't know, a key it can't read, a character
  where words are typed, two actions keys.json gives one key, and the keys it puts ahead of Kumi's own.
- The help (`/help`, `?`), the hints under the box, in the panels, in the map and in setup, and Kumi's notices
  name the keys as keys.json has them.

| Where | Actions, with Kumi's own keys |
| --- | --- |
| The box | `composer.send` (enter), `composer.send-after` (tab, while Kumi works), `composer.take-back-waiting` (alt+↑), `composer.new-line` (ctrl+j, alt+enter, shift+enter), `composer.up`, `composer.down`, `composer.left`, `composer.right`, `composer.word-left` (alt+←, ctrl+←, alt+b), `composer.word-right` (alt+→, ctrl+→, alt+f), `composer.line-start` (home, ctrl+a), `composer.line-end` (end, ctrl+e), `composer.backspace`, `composer.take-back-file` (backspace, in an empty box), `composer.delete` (delete, ctrl+d), `composer.cut-word` (ctrl+w, alt+backspace, ctrl+backspace), `composer.cut-to-end` (ctrl+k), `composer.cut-to-start` (ctrl+u), `composer.yank` (ctrl+y), `composer.yank-earlier` (alt+y), `composer.undo` (ctrl+-, ctrl+_), `composer.history-search` (ctrl+r), `composer.editor` (ctrl+g), `composer.stash` (ctrl+s) |
| Kumi | `app.stop` (esc), `app.take-back` (esc esc, in an empty box), `app.interrupt` (ctrl+c), `app.quit` (ctrl+d), `app.background` (ctrl+b), `app.paste-picture` (ctrl+v), `app.redraw` (ctrl+l), `app.help` (none of its own: `?`), `voice.talk` (ctrl+t), `conversation.page-up`, `conversation.page-down`, `conversation.start` (ctrl+home), `conversation.latest` (ctrl+end), `conversation.details` (ctrl+o) |
| The pane | `pane.toggle` (ctrl+\\), `pane.tabs` (shift+tab), `pane.tree` (tab); in the tabs, `tabs.up`, `tabs.down`, `tabs.page-up`, `tabs.page-down`, `tabs.open` (enter), `tabs.next` (shift+tab), `tabs.leave` (esc, tab); in the tree, `tree.up`, `tree.down`, `tree.pin` (enter), `tree.leave` (esc, tab) |
| The `/` and `@` menus | `menu.up`, `menu.down`, `menu.complete` (tab), `menu.run` (enter; after `@`, it puts in the one chosen, as tab does), `menu.close` (esc) |
| Looking back (ctrl+r) | `search.back` (ctrl+r), `search.erase` (backspace), `search.take` (enter, tab), `search.cancel` (esc, ctrl+c) |
| While Kumi listens | `voice.send` (enter), `voice.drop` (esc, ctrl+c) |
| Selected text | `selection.copy` (ctrl+c), `selection.clear` (esc) |
| The help | `help.up`, `help.down`, `help.page-up`, `help.page-down`, `help.top` (home), `help.end` (end), `help.close` (esc, enter, ctrl+c, `?`) |
| A side answer (`/btw`) | `aside.up`, `aside.down`, `aside.page-up`, `aside.page-down`, `aside.earlier` (←), `aside.later` (→), `aside.copy` (c), `aside.close` (esc, enter, space, ctrl+c) |
| A job's details | `job.up`, `job.down`, `job.page-up`, `job.page-down`, `job.stop` (x), `job.again` (r), `job.copy` (c), `job.close` (esc, enter, space, ctrl+c) |
| A list | `list.up`, `list.down` (↓, tab), `list.next-tab` (tab, where the list has tabs), `list.erase` (backspace), `list.choose` (enter), `list.choose-other` (ctrl+f), `list.close` (esc, ctrl+c) |
| The key box | `key.check` (enter), `key.erase` (backspace), `key.clear` (ctrl+u), `key.cancel` (esc, ctrl+c) |
| ChatGPT's sign-in | `chatgpt.copy` (c), `chatgpt.by-code` (d), `chatgpt.by-browser` (b), `chatgpt.cancel` (esc, ctrl+c) |
| The conversation in detail (ctrl+o) | `details.up`, `details.down`, `details.page-up`, `details.page-down`, `details.top` (home), `details.end` (end), `details.search` (/), `details.next-match` (n), `details.previous-match` (N), `details.next-request` (}), `details.previous-request` ({), `details.print` ([), `details.edit` (v), `details.close` (esc, q, ctrl+c, ctrl+o); while a search is typed, `details.search-erase` (backspace), `details.search-keep` (enter), `details.search-drop` (esc) |
| The map (`/map`) | `map.up`, `map.down`, `map.left`, `map.right` (the arrows), `map.pan-up`, `map.pan-down`, `map.pan-left`, `map.pan-right` (shift and the arrows), `map.zoom-in` (`+`, `=`), `map.zoom-out` (`-`, `_`), `map.play` (space), `map.hover` (h), `map.pick` (x), `map.pick-all` (a), `map.grow` (g), `map.gather` (enter), `map.filter` (`/`), `map.switch` (tab), `map.let-go` (esc), `map.close` (esc, q, ctrl+c); while its filter is typed, `map.filter-erase` (backspace), `map.filter-keep` (enter), `map.filter-drop` (esc) |
| Setup | `setup.up` (↑), `setup.down` (↓, tab), `setup.choose` (enter), `setup.later` (esc), `setup.quit` (ctrl+c, ctrl+d) |

## Plain mode

`KUMI_UI=plain`, or input or output through a pipe, gives a plain line-by-line interface
instead, which suits screen readers and logs. It has the full-screen app's commands, as
text, and `/help` lists those this session has.

- `/btw <question>` answers on the side, and `/goal` and `/loop` work as they do in the
  full-screen app.
- `/jobs`, `/slots` and `/music` print their lists. Changing your music is the
  full-screen app's.
- `/model [provider/model]`, `/effort [level|default]` and `/logout <provider>` take their
  words here, where the full-screen app opens a panel.
- `/export [md|html|json]` and `/share` each save a file and say where.
- `/pin <id>`, `/unpin <id>` and `/forget <id>` keep or forget a note from `/memory`.
- `/voice`, `/map`, `/palette`, `/copy`, `/recipe` and `/takeback` are only in the
  full-screen app, and plain mode says so.
- `/resume` numbers this Set's conversations, then other Sets'. Kumi's question about
  another Set's conversation takes 1, 2 or 3 (commands still run while it waits, and
  other words ask it again).
- It says when a background job starts and ends, and one your request needs carries on
  once Kumi is idle.
- Sign in from a shell with `kumi login`. Ctrl-C stops the answer, or quits when idle.

## Away from Kumi

- **Status in the terminal:** Kumi reports what it's doing with OSC 7501
  (working with NOW's words and a run's progress, waiting for your pick or a
  sign-in, done, failed), which terminals that know it show in a tab or sidebar.
  Windows Terminal, ConEmu and Ghostty also get OSC 9;4 progress. Both clear
  when Kumi quits.
- **While Kumi's window is behind another**, Kumi tells you when it finishes an
  answer of 15 s or more, needs your pick or a sign-in, waits for a plan's limit,
  stops on an error, or ends a `/goal`, `/loop` or match run:
  - a desktop notification: by the terminal's own (Ghostty, WezTerm and foot:
    OSC 777; iTerm2: OSC 9; kitty: OSC 99), else macOS's (Terminal.app and
    others), `notify-send` on Linux, or the bell on Windows Terminal (it flashes
    the taskbar);
  - one line in Live's status bar ("Kumi · done · the bass is darker"), or
    Renoise's; never a dialog, and never in HISTORY. REAPER has no status bar
    Kumi can write to.
  - At most one every 10 s (the newest waiting one goes then); none over a take
    Live is recording, until it stops; none once you're back.
  - A terminal that never says whether it's in front: on a Mac, Kumi asks the
    system which app is, only when there's something to tell.
  - No notification shows? Allow them for your terminal (or, for macOS's own,
    Script Editor) in System Settings › Notifications.
- **The computer stays awake** while Kumi works (an answer, a `/goal`, a wait
  for a plan's limit); the display may sleep.
- **Settings** (`~/.kumi/settings.json`): `"notifications": "off"`,
  `"liveStatusBar": false`, `"keepAwake": false`, `"answerAfter"` (seconds
  before Kumi takes its recommended answer; `0` never), `"theme"`: `system`
  (the default: light or dark as the terminal's background is, turning with it in
  terminals that say when their colours change), `dark` (Kumi's own), `light`
  (for a light terminal) or `colorblind` (colours that stay apart for red-green
  colour blindness), and `"fullRepaint": true` (Kumi writes every cell of its
  window each time something changes, for a terminal that draws only the changes
  wrongly; on by default in Windows Terminal, where `false` turns it off).

## Terminals

Kumi detects how many colours the terminal shows; `KUMI_COLOR` (`truecolor`,
`256`, `16`, `none`) overrides it, and `NO_COLOR` is honoured. Where the
terminal's symbols may not show (the old Windows console, the Linux console),
Kumi uses two-letter badges instead of icons; `KUMI_ICONS=glyphs` or `badges`
chooses. Changes' pictures show as images in kitty and Ghostty (with 24-bit
colour, outside tmux), in braille and half-blocks elsewhere, and as words where
symbols may not show; `KUMI_PICTURES=cells` keeps them in cells. As it
starts, Kumi asks the terminal what it is (its answers turn pictures,
notifications and links on over SSH); `KUMI_PROBE=off` keeps it from asking.
Windows Terminal is recommended on Windows. Web and mail links in Kumi's answers can be
clicked in iTerm2, Ghostty, WezTerm, kitty, Windows Terminal and VS Code's
terminal (outside tmux; some want Cmd or Ctrl held), and there the address no
longer follows the words, unless the words read as another address; elsewhere
it still does. A drag copies the words alone; `/copy` and `/export` keep the
addresses. In terminals without the
kitty keyboard protocol, use Ctrl-J or Alt-Enter for a new line. Inside tmux,
Kumi's status and notifications pass through with `set -g allow-passthrough on`,
focus reports need `set -g focus-events on`, and over SSH Kumi's copies reach
your clipboard with `set -g set-clipboard on`. Those terminals
say when a held Ctrl-T is let go; elsewhere, Kumi stops listening once the key's
repeats stop.

Quitting prints the conversation into the terminal's scrollback before the
goodbye: your requests, Kumi's answers and a line for each run of steps, with
secrets taken out as `/export` takes them out (the last 200 KB of a long one).
`"transcriptOnQuit": false` in settings.json turns this off. `"mouse": "off"`
leaves the mouse to the terminal, or tmux: its own selection and copying work,
Page Up and Page Down scroll, and the wheel types no ↑ or ↓ meanwhile. Kumi's
clicks, its drag to copy and the map's mouse are off then.

## Design notes

For people working on the app (`crates/kumi/src/tui/`).

**Principles.** Kumi owns the whole window and draws every cell, so panes scroll
on their own and stay put. No boxes: areas are separated by background shade.
Colour always means something. Words before symbols, and steps read as music
("looked at Bass and Drums"), never as tool names. Motion only shows that
something is happening or changing. The pane is a view: the runtime describes
what to draw as data, so another front end could draw the same things.

**Palette** (`style.rs`). Greys from `#0e0f12` (ground) to `#f4f6f8` (bright),
one accent, mint `#86e3b5`, and Live's own track colours (very dark ones are
lightened for display). Warning `#e7b45f`, error `#ee8479`, the beat light
`#ffe14d`. What Kumi keeps has a colour per kind: notes `#8cc8ff`, techniques
`#c7a6ff`, recipes `#f2a6c4`, lessons `#e7c88f`. Code in answers is coloured
by the language its fence names (JavaScript and TypeScript, gen~, Lua, Python
and REAPER's EEL): keywords `#7aa2f7`, strings `#b8d47e`, numbers `#e48ee0`,
comments faint, the rest bright on the code's band. Three themes draw it
(`theme`): dark, as above; light, made for a light terminal (dark words on a
warm white `#f7f7f4`, the accent a deeper mint `#14764e`, the warning amber
`#925b0d`, track colours too pale to read deepened, and code's colours deeper:
keywords `#3a4fb8`, strings `#4a6a10`, numbers `#9c2f9a`); and colorblind, the
dark one with colours that stay apart for red-green colour blindness (the accent
sky blue `#56b4e9`, the warning orange `#e69f00`, the error vermilion
`#d7650b`, code's numbers pink `#f0a8d8`). Each theme keeps what its colours
mean, and its words read at 4.5:1 or more. With `system`, Kumi takes light or
dark from the background the terminal reports as Kumi starts, and asks it to say
when its colours turn dark or light, which some terminals do (mode 2031).

**Foundations**, built from terminal primitives with no UI framework:

1. Terminal I/O (`tty.rs`, `probe.rs`): raw mode, the alternate screen,
   bracketed paste, SGR mouse, focus events, the kitty keyboard protocol where
   offered; the terminal is restored on exit, crashes and signals. As the
   screen starts, Kumi asks the terminal once what it is and what its colours
   are, waiting at most a quarter of a second. Its answers choose pictures,
   notifications and colour depth over SSH, where its name doesn't arrive in
   the environment.
2. Input (`keys.rs`): keys with modifiers (xterm and CSI u), pastes, mouse,
   sequences split across reads, and a lone Esc resolved by a short timeout.
   With the kitty protocol, a held key's repeats and its let-go are events of
   their own.
3. Screen and renderer (`screen.rs`, `render.rs`): a grid of cells; each frame
   is diffed against the last and only changed cells are written, inside a
   synchronized update. Colour falls back from 24-bit to 256, 16 and none.
4. Text (`width.rs`, `wrap.rs`): grapheme widths (wide CJK and emoji take two
   cells), wrapping and truncation.
5. Frames (`scheduler.rs`): redraws are coalesced, and an animation clock runs
   only while something moves.

`app.rs` draws the layout and handles input; `editor.rs` is the input box,
`transcript.rs` the conversation, `tree.rs` the device tree and `tabs.rs` the
tabs. Tests replay the renderer's output through a small terminal interpreter,
which must reproduce the intended frame exactly.

**Not done yet:** a picture in NOW for locators and new tracks; a fan-in or
sidechain diagram for routing; an overview strip for very long clips; one
expandable HISTORY entry for a change across many tracks; offering to undo the
changes that depend on one; marking an entry "undone in Live"; FOCUS through
macOS Accessibility (the exact control, the clip tab, the Browser item).
