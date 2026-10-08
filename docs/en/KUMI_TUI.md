# Kumi commands, keys and screens

English · [简体中文](../zh-CN/KUMI_TUI.md) · [日本語](../ja/KUMI_TUI.md)

The reference for Kumi's terminal app: what's on screen, every command and key,
and the plain mode. The [guide](KUMI_GUIDE.md) explains what Kumi does.

## The screen

Kumi takes the whole terminal window and gives it back as it was when Kumi
closes, crashes or is stopped.

- **Header:** Kumi, the Set's name, the model and its effort, and Live's
  connection. While Live plays, a yellow light blinks on its beat (brightest on
  the bar's first beat) beside the tempo.
- **Conversation** (left): your messages, Kumi's answers and its steps, in plain
  words ("looked at your Set"), each with its timing. A step at work animates by
  kind (searching, reading, building a device, changing, listening, playing);
  the same step done several times folds into one line ("read a page ×3").
  What Kumi heard shows as a small spectrum, and what it keeps as a line of its
  own: ✎ a note, ◆ a technique, ↻ a recipe, ✦ a lesson from matching.
- **FOCUS** (top right): where you are in Live, following your selection as it
  changes: a track's device tree with rack chains, a clip's notes as a small
  piano roll, or a Session or Arrangement strip. A device you point at (click
  it, or use **Ask Kumi about this** in Live) is pinned for your next messages.
- **NOW** (middle right): what Kumi is doing, drawn as it happens: a value's
  before and after, a new clip's notes, a colour swatch, a device landing in its
  chain, "▶ Playing from the start marker".
- **Tabs** (bottom right): **HISTORY** lists the latest three things Kumi kept
  (each with **forget**), then every change, newest first, each with **undo**,
  or **kept** / **no undo** / **check Live** when it can't be undone. **GOAL**
  shows the `/goal` ("No goal yet" until there is one): the goal, its turns and
  time against the budget, the last check and what's next; for a sound-match
  `/loop`, the best score with its trend, the leading candidate, and the time.
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

**Setup** comes first when a step is missing: signing in, Kumi's bridge in Live
(missing, or older than Kumi's), and then the Control Surface. It lists the three
steps, done ones in mint with what was chosen, and asks what the current one
needs; a mint waveform under the wordmark moves while Kumi waits (for the
browser, the install, or Live). ↑↓ and Enter choose, Esc leaves the step for
later, and the session follows in the same window. A Kumi that's set up starts
straight into the session.

Below 100 columns the Live pane folds into a two-line strip above the input box
(where you are and what Kumi is doing, then the last change with its undo).
Below 24×8 Kumi asks for a bigger window.

## Commands in a terminal

Run these as `kumi <command>`; from a copy of the repository, as
`npm run kumi -- <command>`.

| Command | What it does |
| --- | --- |
| `kumi` | Open Kumi on the Set Live has open |
| `kumi --inference-only` | Chat without Live |
| `kumi --bridge-config <absolute path>` | Use your own bridge configuration |
| `kumi login` | Sign in: asks whether with ChatGPT or an API key |
| `kumi login <provider>` | Sign in to `openai-codex` (ChatGPT; `--device` without a browser), or `anthropic`, `openai`, `opencode` or `opencode-go` with an API key |
| `kumi logout <provider>` | Remove Kumi's sign-in there |
| `kumi model [<provider>/<model>]` | Show or choose the model, a model on your computer too (`ollama/<model>`) |
| `kumi auth` | Which providers are usable, the model and the sign-in file (never secrets) |
| `kumi bridge [--yes] [--allow-dirty]` | With Live closed: put the bridge into Live, or bring it up to date. `--yes` confirms Live is closed; `--allow-dirty` lets a checkout with uncommitted changes install its bridge |
| `kumi doctor` | Check Node, sign-in, the model servers on your computer, the bridge, Live, the extension, your library, video programs, talking, Live's menus and the terminal; says what to run |
| `kumi library [--rebuild]` | How far Kumi has got learning your sounds, presets and Sets; `--rebuild` learns them all again |
| `kumi update [--check \| --rollback]` | Get the newest Kumi (and the bridge when it's older); `--check` only asks; `--rollback` goes back to the one before, with its bridge when Live is closed (an installed Kumi only) |
| `kumi report` | Write `~/kumi-report-<date and time>.txt` to send when something goes wrong |
| `kumi uninstall [--all] [--yes]` | Remove an installed Kumi; `--all` also removes conversations, notes, recipes and sign-ins; `--yes` skips the first question |
| `kumi --version` (or `-v`), `kumi --help` | Version; help |

## Commands inside Kumi

Type `/` for a menu of these: ↑↓ choose, Tab completes, Enter runs, Esc closes.
A message that starts with a path (a file dragged into the terminal) is a
message, not a command.

| Command | What it does |
| --- | --- |
| `/new` | Start a fresh conversation. What's above stays on screen under a line, the last conversation is kept, and HISTORY's undo still works |
| `/btw <question>` | Ask something on the side, any time, even while Kumi works: answered from the conversation so far, without tools, in a panel, and not added to the conversation. `/btw` alone shows the last answer again |
| `/conversations` | This Set's kept conversations (the latest 20); choose one to carry on with it |
| `/reconnect` | Connect to Live again over a fresh bridge, keeping the conversation (earlier changes lose Kumi's undo) |
| `/undo` | Undo Kumi's latest change |
| `/stop` | Stop Live: clips, the transport and recording. Also stops Kumi's answer |
| `/refresh` | Read the Set again without asking the model |
| `/copy` | Copy Kumi's last answer to the clipboard |
| `/model`, `/effort` | Choose the model (from each provider's own list, then the model servers on your computer: Ollama, LM Studio and those in settings.json; type to filter) and how hard it thinks; from your next message |
| `/fast` | Turn on the model's faster tier when its provider lists one (ChatGPT's "Fast": quicker answers, more of your usage); `/fast` again turns it off. Shown as "· fast" beside the model |
| `/willington` | Turn [Willington](WILLINGTON_INTEGRATION.md) on or off. Rack bindings are requested on supported builds; clip Follow Actions and native editing also require passing exact-library self-tests. Native editing is b5 macOS ARM64 only. Off until enabled; available when the bridge carries Willington. |
| `/login`, `/logout` | Sign in (ChatGPT in the browser, or an API key shown only as dots) or out |
| `/loop <what to reach>` | Work in judged rounds until it's met: one change, heard before and after, kept only if nothing else got worse, then the next target; Esc or `/loop stop` stops it. A sound to match ("make my pad sound like ~/ref.wav") runs the knob search instead: `/loop` alone picks a paused one up, `/loop stop` ends it |
| `/goal <what to reach>` | Keep at one goal across turns, checked after each (by the judge's numbers when measured), until it's met, blocked or out of budget. `/goal` shows it; `/goal resume`, `/goal edit <words>`, `/goal new: <words>` (replaces an unfinished one), `/goal pause` (or Esc), `/goal clear` |
| `/memory` | Everything Kumi keeps: notes about you and this Set, what it learned from your Sets, techniques, recipes and lessons; choose one to forget it (a note to change its words or pin it, a recipe to run or forget) |
| `/note <id> <new words>` | Change a note's words without the model; a note's "Change the words" in `/memory` starts it for you |
| `/recipes` | Your recipes: run one or forget it. One with blanks starts a `/recipe` line for you to finish, with what's pinned filled in |
| `/recipe <name> blank=value …` | Run a recipe now, with no model call; a value with spaces goes in quotes, an unquoted number or true/false goes as itself (quote it to keep it words), and Kumi names any blank left empty |
| `/status` | What Kumi is connected to, the model, how far it has got learning your library, and on an API key the tokens this session used |
| `/voice` | Talking instead of typing: start or stop, "Send when you stop", the language you speak and the microphone |
| `/update` | Get the newest Kumi: it asks, closes, updates and opens again with the same conversation |
| `/changelog` | What's new in Kumi: every note since the version you updated from, or the latest three releases, and where the whole history is |
| `/help` | The keys and commands, as a note in the conversation |
| `/quit` | Close Kumi |

`/new`, `/reconnect`, `/refresh` and `/undo` work only while Kumi isn't
answering; during an answer, Kumi says so and leaves things as they are.

## Keys

**Typing and sending**

| Key | Action |
| --- | --- |
| Enter | Send. While Kumi works, it reads the message after the step under way |
| Tab, while Kumi works | Send the message after the answer instead; it waits above the box |
| Alt-↑ | Take the last waiting message back into the box |
| Ctrl-J, Alt-Enter, Shift-Enter | New line (Shift-Enter in terminals that report it) |
| ↑ and ↓ | Move through the box's lines, then through what you sent before (kept across `/new` and restarts, without secrets) |
| Ctrl-A / Ctrl-E, Home / End | Start / end of the line |
| Alt-← / Alt-→, Ctrl-← / Ctrl-→, Alt-B / Alt-F | Word left / right |
| Ctrl-W, Alt-Backspace, Ctrl-Backspace | Delete the word before the cursor |
| Ctrl-K / Ctrl-U | Delete to the end / start of the line |
| Ctrl-T | Talk instead of typing: press it again to stop, or hold it while you talk. What you said lands in the box at the cursor; Enter stops and sends at once, Esc drops it |
| Ctrl-V | Add the picture on the clipboard (a screenshot, say) to your next message; files dragged into the window are added the same way |
| Backspace, in an empty box | Take back the last file added |

**Stopping and moving around**

| Key | Action |
| --- | --- |
| Esc | Close the `/` menu; otherwise stop Kumi's answer (the steps it finished stay, waiting messages go back into the box); otherwise unpin the pinned device |
| Ctrl-C | Stop Kumi's answer; when idle, clear the box; with an empty box, quit |
| Ctrl-D | Quit, when idle with an empty box |
| Page Up / Page Down, mouse wheel | Scroll the conversation (the wheel over the tabs scrolls the tab); it stays put while new text arrives |
| Ctrl-Home / Ctrl-End | The start of the conversation / back to the latest |
| Ctrl-L | Redraw the screen |

**The Live pane**

| Key | Action |
| --- | --- |
| Tab, when idle | Into FOCUS's device tree, at Live's selection: ↑↓ move, Enter points at the row, Esc or Tab goes back to typing |
| Shift-Tab | Into the tabs (again for the next tab): ↑↓ and Page Up/Down move, Enter does the row's **undo** or **forget**, Esc or Tab goes back |
| Mouse | Click **undo** or **forget** at a row's end, a tab's name, or a device in the tree to point at it; click the pin to unpin. Hold Shift while dragging (Option in iTerm2) to select text |

**Panels** (`/model`, `/effort`, `/login`, `/memory` and the others): ↑↓ move,
Enter chooses, Esc closes; in `/model`, `/memory`, `/conversations` and
`/recipes`, typing filters the list. In the key box, paste the key and press
Enter; Kumi checks it with the provider, or saves it unchecked when the provider
can't be reached. In the ChatGPT sign-in panel, `c` copies the link. In the
`/btw` panel, ↑↓ and Page Up/Down scroll, ←→ go through earlier answers, `c`
copies, and Esc, Enter or Space closes it.

## Plain mode

`KUMI_UI=plain`, or input or output through a pipe, gives a plain line-by-line interface
instead, which suits screen readers and logs. It has `/help`, `/status`,
`/undo`, `/stop`, `/refresh`, `/reconnect`, `/new`, `/conversations [n]`,
`/model [provider/model]`, `/effort [level|default]`, `/fast`, `/logout <provider>`,
`/memory`, `/forget <id>`, `/note <id> <new words>`, `/pin <id>`, `/unpin <id>`,
`/recipes`, `/willington`, `/update`, `/changelog` and `/quit`, but no `/btw`,
`/goal`, `/loop` or `/copy`. Sign in from a shell with `kumi login`. Ctrl-C stops the
answer, or quits when idle.

## Terminals

Kumi detects how many colours the terminal shows; `KUMI_COLOR` (`truecolor`,
`256`, `16`, `none`) overrides it, and `NO_COLOR` is honoured. Where the
terminal's symbols may not show (the old Windows console, the Linux console),
Kumi uses two-letter badges instead of icons; `KUMI_ICONS=glyphs` or `badges`
chooses. Windows Terminal is recommended on Windows. In terminals without the
kitty keyboard protocol, use Ctrl-J or Alt-Enter for a new line. Those terminals
say when a held Ctrl-T is let go; elsewhere, Kumi stops listening once the key's
repeats stop.

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
`#c7a6ff`, recipes `#f2a6c4`, lessons `#e7c88f`.

**Foundations**, built from terminal primitives with no UI framework:

1. Terminal I/O (`tty.rs`): raw mode, the alternate screen, bracketed paste,
   SGR mouse, focus events, the kitty keyboard protocol where offered; the
   terminal is restored on exit, crashes and signals.
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
