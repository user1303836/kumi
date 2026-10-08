# Kumi guide

English · [简体中文](../zh-CN/KUMI_GUIDE.md) · [日本語](../ja/KUMI_GUIDE.md)

Kumi is a producer agent for the Ableton Live Set you have open, in your
terminal. It reads the Set, makes the changes you ask for and shows each one in
HISTORY. It plays, records and renders, listens to audio and compares it with a
reference, watches video tutorials, makes Max for Live devices, looks things up
on the web, and remembers how you work.

To install Kumi and connect it to Live, follow [Quickstart](../../README.md#quickstart).
This guide covers everything after that. For the keys and the screen, see
[commands, keys and screens](KUMI_TUI.md); for how changes and undo work, see
[how Kumi changes your Set](KUMI_CHANGES.md).

## Sign in and choose a model

Inside Kumi, `/login` signs in (the first time, Kumi's setup asks first): to
ChatGPT with your plan (the browser opens and
the sign-in comes back on `localhost:1455`), or to Anthropic, OpenAI or OpenCode
with an API key, pasted into a box that shows only dots. Kumi checks a key with
its provider before keeping it; if the provider can't be reached, it keeps the
key and says it isn't checked yet. `/logout` signs out.

| Provider | Model names | Sign-in |
| --- | --- | --- |
| ChatGPT plan | `openai-codex/<model>` | `/login` (browser), or `kumi login openai-codex` (`--device` on a machine without a browser) |
| Anthropic API | `anthropic/<model>` | `/login` with an API key, or `ANTHROPIC_API_KEY` |
| OpenAI API | `openai/<model>` | `/login` with an API key, or `OPENAI_API_KEY` |
| OpenCode Zen and Go | `opencode/<model>`, `opencode-go/<model>` | `/login` with an API key (one for both), or `OPENCODE_API_KEY` |
| Ollama, on your computer | `ollama/<model>` | None: found while it runs |
| LM Studio, on your computer | `lmstudio/<model>` | None: found while its server runs |
| Another OpenAI-compatible server | `<name>/<model>` | Named in `settings.json` ([below](#models-on-your-computer)), with a key if it wants one |

A key saved with `/login` is used first; without one, Kumi uses the key in the
environment, and can't sign out of that (unset it instead). Paste an API key
only into Kumi's key box or its `kumi login` prompt, never into a message.
Claude and Gemini subscription sign-ins aren't offered, because their providers
don't permit them in third-party tools. OpenCode's Gemini models aren't
supported yet.

`/model` lists each provider's models, read from the provider itself, so a model
released today is there without a Kumi update. With no model chosen, Kumi uses
the first model of the first provider you're signed in to (in the order of the
table) and says which. `/effort` sets how hard the model thinks, from the levels
that model offers; lower answers sooner. Both apply from your next message and
are kept for next time. Changing model keeps the conversation, except the
earlier model's private reasoning, which belongs to that model.

When an answer fails because a sign-in is missing or was refused, Kumi offers to
sign in and then sends your message again. When the provider doesn't offer the
chosen model, Kumi offers to choose another. When an answer breaks off partway
(a dropped connection, say), Kumi carries on once from where it stopped and says
so in the status line; if it breaks off again, Kumi offers to send your message
again.

### Models on your computer

- **Ollama** and **LM Studio** are found while they run, with no sign-in.
  `/model` lists each as its own provider ("Ollama · on this computer") with the
  models it has; one that's installed but closed says how to start it. Signed in
  nowhere, Kumi starts with one of their models that can change the Set,
  preferring one already loaded.
- **Another OpenAI-compatible server** (llama.cpp's `llama-server`, vLLM, Jan…)
  goes in `~/.kumi/settings.json`. Its name becomes its id (`llama.cpp` is
  `llama-cpp/<model>`):

  ```json
  { "modelServers": [
    { "name": "llama.cpp", "baseURL": "http://127.0.0.1:8080/v1" },
    { "name": "Studio PC", "baseURL": "http://192.168.1.20:8000/v1", "apiKey": "…" }
  ] }
  ```

- **Room for Kumi.** Its instructions and tools take about 25–30k tokens. Kumi
  asks Ollama for room for them, the conversation and an answer (about 57k
  tokens, up to what the model reads at most), and has LM Studio load a model
  with that room when it isn't loaded with enough. The room takes memory, often
  more than the model itself.
- **A model that can't use tools** still talks about the Set but can't change it;
  Kumi says so once and names one on the same server that can. `kumi doctor`
  lists the servers it finds.

## Connect to Live

Kumi reaches Live through its bridge: a Remote Script that runs inside Live and
a local MCP server that Kumi starts. The first time you open Kumi, and whenever
Kumi's bridge is newer than Live's, Kumi's setup runs the steps still missing,
in the app:

1. **Sign in**, if Kumi isn't signed in yet.
2. **Connect to Live**: Kumi puts the bridge in place and opens Live. If Live is
   open, choose **Restart Live now** (Kumi asks Live to quit, and Live asks you
   to save your work first) or **I'll quit it** (Kumi waits while you quit
   Live). If Live is still open a while after Kumi asked (you chose Cancel when
   it asked about saving), Kumi says so and offers to ask again. Once Kumi has
   Live closed, Live opens again however the step ends.
3. **Control Surface**: the first time, open **Settings → Link, Tempo & MIDI** in
   Live and choose **AbletonMcpBridge** as a Control Surface. Kumi notices by
   itself, and Live remembers the choice.

Esc on a step leaves it for later: Kumi chats without Live for now. Sign in and
Connect to Live are offered again next time; past Connect to Live, Kumi connects
by itself whenever Live answers. Quitting Kumi while the bridge goes in waits
until it's in place.
`kumi bridge` does the same from a shell, with Live closed: it asks you to
confirm (`--yes` confirms beforehand), refuses while Live is running, and never
quits or starts Live. Both install or update the bridge through the bridge's own
lifecycle, with checks, receipts and rollback, and keep its settings and secret
across updates. `kumi bridge` waits up to ten minutes for Live to connect, and
says when it does. `kumi update` runs it for you when
the bridge in Live is older than Kumi's and Live is closed; `kumi doctor` says
when to run it yourself.

Kumi finds Live's Remote Scripts folder in your User Library, including one
you moved elsewhere (it reads Live's own settings for that).
`KUMI_REMOTE_SCRIPTS_DIR` overrides it.

**Kumi's Live extension.** On Live 12.4 and later, `kumi bridge` also puts Kumi's
extension into Live's Extensions folder (on macOS
`~/Library/Application Support/Ableton/Extensions/kumi.kumi`; on Windows Kumi
uses `%LOCALAPPDATA%\Ableton\Extensions`, which isn't confirmed on Windows yet).
Live starts it the next time it opens. The extension writes MIDI clips straight
into the Arrangement, clears a stretch of a track, renders a track's clips
without playing them, and adds **Ask Kumi about this** to Live's right-click
menu (under **Extensions**), which pins what you clicked for your next message.
With Developer Mode on (Settings → Extensions), Live starts no extensions, so
the bridge starts Kumi's itself. Kumi works without the extension, and
`kumi doctor` says whether it's there and running.

Once the bridge is installed and chosen in Live, `kumi` finds it and connects;
there is nothing to configure. Without it, Kumi starts anyway, chats without
Live (**No Live access**) and offers to connect. `kumi --bridge-config
<absolute path>` uses a bridge configuration of your own; `kumi
--inference-only` chats without Live.

**Willington.** Willington's native bindings come in Kumi's bridge from the
release that carries their files; until then, you can install Willington
yourself. Either way they're off until you turn them on with `/willington`.
Then Kumi can also map rack macros, set
chain zones and, with a passing self-test, edit Follow Actions, on the Live versions Willington has
bindings for: Live 12.4.15b4 and b5 on macOS ARM64 (chain zones on b5 only) and
Live 12.4.15b5 on Windows x64. See [Willington](WILLINGTON_INTEGRATION.md).

## Working with Kumi

Kumi runs full screen: the conversation on the left, a Live pane on the right
and the input box at the bottom. FOCUS follows what you touch in Live, NOW shows
what Kumi is doing, and HISTORY lists every change with its undo. Below 100
columns the Live pane folds into a strip above the input box. `KUMI_UI=plain`,
or piping the output, gives plain line-by-line output instead, which suits
screen readers. [Commands, keys and screens](KUMI_TUI.md) has the details.

Try "Describe the open Set: tracks, tempo and transport", then ask which devices
are on a track. Ask for a change, such as "Set the tempo to 124 and rename
3-Audio to Bass": each change appears in HISTORY with **undo** beside it. Click a
device in FOCUS, or right-click something in Live and choose **Ask Kumi about
this**, to point at it: "this Saturator's too harsh".

While Kumi works, Enter tells it more (it reads your message after the step
under way), Tab sends a message for after the answer, and `/btw` asks something
on the side without interrupting. Esc stops the answer; the steps it finished
stay. `/stop` stops Live (clips, the transport and recording) at any time.

When Kumi asks you to pick (which track, which version), its options appear
above the input box: press its number and Enter, or just type your own.

To show Kumi something, drag files into the window or press **Ctrl-V** for a
picture on the clipboard, such as a screenshot of a synth: "make this". Each
shows above the input box and goes with your next message; × or Backspace in an
empty box takes one back. The model sees PNG, JPEG, GIF and WebP pictures, up to
3.75 MB each and 20 MB in one message. Other files, such as reference audio, a preset or a Live Set, go by their
path for Kumi to use. A picture goes with its own message only: after it, the
conversation, saved or not, keeps each file's name and path but not the picture.
Clipboard pictures are kept in `~/.kumi/attachments` for a week.

## Talking to Kumi

Press **Ctrl-T** and say what you want, then press it again. What you said lands
in the input box; Enter sends it.

- **Hold to talk.** Hold Ctrl-T while you talk, and let go to stop. Terminals with
  the kitty keyboard protocol (kitty, Ghostty, WezTerm, iTerm2) say when it's let
  go; elsewhere Kumi follows the key's own repeats.
- **While Kumi listens**, the input box shows a pulsing dot, the time and a level
  meter. Enter stops and sends at once; Esc drops it. After you've spoken, 3
  seconds of quiet stops it by itself. Two minutes at most at a time.
- **`/voice`**: start or stop; send what you say as soon as you stop, without
  Enter; the language you speak (English, your computer's, or any); the
  microphone.
- **Private.** ffmpeg hears the microphone and whisper.cpp writes down what you
  said, on your computer; the recording is deleted as soon as it's written down.
- **What it needs:** ffmpeg and whisper.cpp (`brew install ffmpeg whisper-cpp` on
  a Mac; Kumi fetches them on Windows and Linux), and a speech model Kumi fetches
  the first time you talk (about 190 MB, shared with video watching), with a
  small voice-activity model that keeps music and noise from becoming words.
  `kumi doctor` says whether talking is ready.
- **The first time on a Mac**, macOS asks whether your terminal may use the
  microphone. When Kumi can't hear you, it says why (no permission, only silence,
  only quiet, no words) and offers the fix: the privacy settings, or another
  microphone.
- Talking is in the full-screen app; plain lines (`KUMI_UI=plain`) are typed only.

## Changes and undo

Kumi plans a request as one set of changes and runs it at once. Every change
lands in HISTORY with a title in plain words ("Tempo 120 → 124 BPM"). Click
**undo** beside it, type `/undo` for the latest, or ask Kumi. Undo restores
exactly what the change replaced. When it can't (the object is gone, or for
some settings, you changed the same thing again since), the row reads **kept**
with the reason. A whole plan is also one step in Live's own undo, so one Cmd-Z
in Live (Ctrl-Z on Windows) takes it back.

Some changes can't be taken back by Kumi (deleting a track, cropping a clip,
adding a rack chain). HISTORY marks them **kept**, and Live's own undo still
takes them back. Kumi deletes things when you ask, or when the request implies
it ("start over", "replace the drums").

**A copy before big changes.** When a plan reaches its third step, or a step
that deletes something, Kumi copies the Set as last saved next to it
(`Song.backup-<date>.als`) and says so, once for each saved version. Unsaved
work isn't in the file, so it isn't in the copy, and an unsaved Set gets none.

**Python in Live.** For what its other tools can't reach, Kumi can run Python
inside Live with Live's own API. A script's changes are one step in Live's
undo, but they get no HISTORY entry; Live's undo takes them back.

**What Live doesn't let scripts do:** map a macro or a modulator to a parameter
(Willington can map both: `/willington`), or edit the Arrangement's automation lanes. Kumi
says so and suggests a way round. Saving, exporting, freezing, bouncing and
grouping go through [Live's own commands](#lives-own-commands).

[How Kumi changes your Set](KUMI_CHANGES.md) lists every change Kumi can make.

## Playing, recording and rendering

Kumi plays and stops the Set, launches clips and scenes, moves the playhead and
records, when you ask or when it helps to check or show what it built. If a plan
stops partway (a step fails, or you press Esc), Kumi stops the playback and
recording it started. If Live refuses its ordinary stop, Kumi uses the bridge's
emergency stop.

Live gives scripts no bounce, so Kumi bounces by resampling: it adds an audio
track fed from the source track, or from "Resampling" for the whole mix, records
the length you want in the Arrangement, and disarms the track. The recording
stays in the Set as an audio clip. With the extension, Kumi can also render an
audio track's own clips to a file without playing them (before the track's
devices). Live's own Bounce to New Track and Bounce Track in Place work too,
through [Live's own commands](#lives-own-commands).

## Live's own commands

Some things Live's scripting doesn't offer at all. For those, Kumi uses Live's
own menus, as you would: grouping and ungrouping tracks; freezing, unfreezing
and flattening; bouncing without playing (Bounce to New Track, Bounce Track in
Place); consolidating; converting audio to MIDI (melody, harmony, drums);
separating stems; slicing to a MIDI track; saving the Set, or collecting all and
saving; exporting audio or a MIDI clip.

Kumi also works with Set files through Live's File menu: it starts a new Set,
and on Windows it also saves the Set under a name and folder you give (Save Live
Set As) or opens one by its file. On a Mac Kumi can't fill in Live's Save and
Open dialogs yet, so it asks you to do those two in Live. When the open Set has
unsaved changes, Live asks first; Kumi saves or discards them as you said, and
asks you when you didn't. If a file is already where a Set is to be saved, Live
asks before replacing it, and Kumi leaves that answer to you. Live keeps a Set in
a project folder: saved under a new name in a folder that isn't a project, it
makes "<name> Project" there, and Kumi says where the Set went. Live drops Kumi for
a moment while a Set opens, and the request that asked for it carries on in the
new Set. Kumi never writes `.als` files itself.

- Kumi selects what the command works on, presses it, and says what changed.
  When Live opens a dialog (Export, say), Kumi reads it and answers it.
- Tracks are selected by name through the accessibility Live 12 offers screen
  readers. Live stays where it is: nothing comes to the front, and a command
  takes well under a second (a bounce or a freeze as long as Live takes to
  render).
- A track already frozen isn't frozen again: Live's command would undo it, so
  Kumi checks first.
- Clip commands work on a Session clip, or on the clip you've selected in Live.
  Kumi can't select a clip in the Arrangement yet.
- HISTORY lists each command, and Live's own undo (Cmd-Z) takes it back.
- **On a Mac** this uses Accessibility. The first time, macOS asks: turn on the
  app Kumi runs in (your terminal) in System Settings › Privacy & Security ›
  Accessibility. `kumi doctor` says whether it's on. **On Windows** it reads
  Live's menu bar and dialogs through Windows itself (and selects tracks through
  UI Automation), and needs nothing set up; `kumi doctor` checks it can read
  Live's menus. On Windows, Live's prompts say Yes, No and Cancel where a Mac
  says Save and Don't Save: Kumi takes either.

## Listening

Kumi hears audio files and the Set's audio clips: a reference track, a sample, a
bounce or a recording. It measures:

- loudness: integrated LUFS, true peak and loudness range;
- tonal balance in ten bands, from sub to air, and stereo width in each;
- dynamics, tempo and key;
- for a single sound, its pitch, harmonics, envelope and movement (an LFO's
  rate, at the tempo);
- for a part with notes, the notes themselves (from its first minute), to write
  as a MIDI clip.

Given a reference, it matches the loudness and says what differs most. The
conversation shows what it heard as a small spectrum, and a comparison as dB
over or under the reference. Kumi reads WAV and AIFF itself, and MP3, M4A, FLAC
and the like through macOS's `afconvert` or, elsewhere, `ffmpeg` (which Kumi
fetches on Windows the first time it's needed). The analysis runs on your
computer: only the numbers go to the model, never the audio.

**Hearing the Set.** Ask about a track or the mix ("is the bass muddy?", "what
clashes with the kick?") and Kumi hears it in Live directly, with nothing to set
up:

- **While Live plays**, it listens to what's playing for a few seconds, and
  leaves Main and the transport alone.
- **While Live is stopped**, it plays the loop (or a few bars from the playhead,
  the part you name, or the whole song to its last clip) with Main silenced, and
  puts Main back. A listen takes as long as the music it hears.
- **Several tracks at once:** each one's sound, and where two sit in the same
  band at similar levels.
- **How:** Kumi Ears, a small Max for Live device Kumi brings (`kumi bridge` puts
  it in your User Library's Kumi folder). Kumi places it at the end of a track's
  chain when it needs to hear it and takes it away after. Sound passes through
  it untouched, and nothing is recorded into your Set.
- Auditions and goals hear their candidates the same way: no scratch tracks, no
  arming. Without Max for Live, Kumi records to listen instead.

## Judging changes and the loop

For work that can be measured (mastering to a loudness, taming a harsh
resonance, making the vocal cut through), Kumi judges its own changes instead of
trusting them:

- **A goal becomes a checklist:** a loudness and a ceiling, a balance, a
  reference's profile, or the element that must cut through, each with a
  tolerance. Kumi first hears the whole stretch and lists what's off: harsh or
  resonant frequencies, wide lows, rumble, DC, a loud bass note, overs and
  clipping, and what masks the element you name.
- **One change at a time:** after each change Kumi hears before and after at the
  same loudness, and keeps the change only if its target improved and nothing
  else got audibly worse (a measure it can no longer read, such as silence,
  counts as worse). Otherwise Kumi's own undo takes it back. When a kept change
  moved the level, the last limiter or Utility evens it out and Kumi hears it
  again: if the peaks come back or it clips at that level, the change goes too.
- **Code picks the numbers.** Kumi chooses the change; code finds its values,
  with as few listens as it can: an EQ calculated from what was measured (the
  smallest cut that clears a resonance, or bands toward a reference's shape),
  one knob homed in on (a limiter's gain until the loudness is right, a
  de-esser's threshold), or a few knobs that interact searched in small
  generations, each heard side by side on scratch copies of the track in one
  pass. Every added device and every move has to earn its place.
- **The round log** shows each round in the conversation: what it was after, the
  change, the numbers before → after, kept or taken back and why, and what's
  next, with the listens so far. A target that resists two changes in a row
  waits while Kumi works on the next one.
- **A listening model**, when there is one, hears before and after too, and can
  turn down a change it hears as harsh, muddy or distorted; the meters decide
  anything at the dB level. With a Gemini API key (`GEMINI_API_KEY`) it's
  Gemini's newest model, picked from Gemini's own list (it hears a mono mix, so
  it isn't asked about width or air); else, with an OpenAI API key, OpenAI's
  newest audio model. `KUMI_LISTENER` names another (`<base URL>#<model>`, such
  as a model server on your computer), and `KUMI_LISTENER=off` leaves the meters
  alone.
- **Model slots.** `/slots` shows which model does each listening job: stems
  (Live's own splitter), transcription (Live's conversions), listening (the
  lookup above) and embeddings (none yet). Swap one in plain words, such as
  `/slots listening gemini`, `/slots listening off` or
  `/slots listening http://127.0.0.1:8080/v1#<model>`. A new listening model is
  tried on a known clip first, and switched to only if it hears it right.
  `/slots back listening` takes a swap back. Kumi can't run model files itself
  yet, so a file or a Hugging Face link is turned down with what to do instead.
  `KUMI_LISTENER` wins over the listening slot while it's set.

**References** become targets too. Give Kumi a file, a folder of them, a YouTube
video or playlist, a Spotify link, or just words ("like Burial", "dub techno",
"Kid A"): it finds example tracks (an artist's best-known songs, an album's
tracks, a genre's main artists), measures each (passing over silent tracks and,
in a folder or a search, sketches under 30 seconds), and keeps a profile with the
range the tracks keep to, so "in the style" means inside it. When the words could
mean more than one thing (two artists by one name, or an album and an artist),
Kumi asks you once which you mean, and your answer settles it. Each reference is
measured once and kept in `~/.kumi/references`; a file or folder is measured
again once its files change. For one element of a reference
(its bass, its drums), Kumi can put the track in your Set, separate its stems
with Live's own splitter, and measure a stem.

**A part's feel** is judged on its notes, with no listening: where each drum
(or the part) plays in the bar, how early or late each step sits (its push and
swing), its accents, syncopation and fills, against a reference's MIDI. For a
record's drums, Kumi separates its stems, converts the drum stem to MIDI with
Live's Drums to MIDI, and moves each hit onto the stem's real onset first. Kumi
can move your notes toward the reference itself, step by step, and keeps a
change only if it got closer without anything else moving away.

**A song's form** is heard bar by bar: the energy curve (loudness, density,
brightness, low end), the sections with their roles and repeats, the turns
between them and what prepares each drop, the intro, outro and first hook, and
which tracks play where. Kumi also points out what doesn't work: sections without
contrast, an energy plateau, odd phrase lengths, a drop that arrives unprepared,
a loop left unchanged for 32 bars or more. Against a reference song, it says how
the two forms differ.

**A sound** is measured beside the others in one quiet pass: its attack, its
decay against the beat, brightness, an 808's or a kick's pitch drop, a wobble's
rate in beats, warmth (2nd and 3rd harmonics), width, top, noise floor, how long
its tail hangs on, and crackle. Kumi points out clicks at note edges, DC and
notes that jump out. A kit is judged as one: a piece whose noise floor, top or
grit sits apart, pieces that pile up or leave a gap, decays ringing into the next
hit. Against a kick, it says how far each part ducks and how fast it comes back,
and whether their hits interlock or collide; with the key, how far each part's
notes sit from it. Effects are read off what's heard too: a reverb's decay time
and how its tails darken and widen, echoes as a note value with their repeats
and how far each falls, how deep a swing goes, how far a filter sweeps, pumping
against the beat, and how much of the sound is tail, bar by bar. Kumi flags tails
cut off, echoes out of time, repeats that don't die away and tails burying the
dry hits. Ask for a sound or an effect like a reference and Kumi closes those
measured gaps round by round, keeping only what got closer.

`/loop` and what to reach works in rounds until it's met: one change, judged,
then the next target. Kumi decides when it stops, not the model: every item
within tolerance, changes no longer helping, or 16 rounds or 45 minutes. Its
checklist stays as first set, and the model ends the run only when Kumi asks:
one more listen to the whole stretch and what changed. Kumi starts the same loop
by itself when the model judges a change of its own (measuring alone starts
nothing). Esc or `/loop stop` stops it, and so does a message of yours once it's
answered; what's kept stays.

`/goal` and what to reach keeps at one goal across turns until it's met. After
every turn Kumi checks it: by the judge's numbers when it measured, otherwise by
the model's own check, which must say complete, blocked (and what you must do
first) or continue (and the next step). It stops at 12 turns, an hour, or three
turns in a row that got no closer (by the judge's numbers, a step or more;
otherwise, a change in the Set), saying how far it got. An error you must fix
(sign-in, billing, the model, a setting, Live) stops it as blocked, naming what
to do. A message of yours while it runs is answered, then the goal waits for
you. The GOAL tab shows the goal, its turns and time against the budget, the
last check and what's next. `/goal` alone shows it; `/goal edit <words>` changes
it, `/goal resume` carries on (with a fresh budget), `/goal pause` or Esc pauses
it, and `/goal clear` ends it. Another `/goal` while one is unfinished asks
first: send it again, or `/goal new: <words>`, to replace it. A goal is kept with
its Set, so it's still there after a restart. An unsaved Set has no name to keep
it by: its goal stays with that conversation, and moves with the Set's first
save.

## Matching a reference

Ask Kumi to make something sound like a reference ("make the bass sound like
this: ~/refs/bass.wav") and it treats it as a search, not a guess. It listens to
the reference, builds two to four different takes on their own tracks, and
renders them together quietly to score each against the reference (0 to 100,
with the biggest differences). It then refines the best, changes structure when
no knob closes a gap, and stops when it reaches the target, when new ideas stop
helping, or after 12 rounds or 45 minutes. It ends with the score before and
after and what still differs.

`/loop` and a sound to match goes further: Kumi keeps searching, mostly with
its own fast knob search and with the model's bigger ideas every few
generations, until the score reaches 95, you stop it, or four hours pass. The
GOAL tab shows how it's going. Esc pauses the search, `/loop` alone picks it up
again (even after a restart), and `/loop stop` ends it.

What Kumi learns from each match is kept as a lesson (✦) for the next one;
`/memory` lists them.

## Arrangements

Ask Kumi to turn a loop into a track: "arrange this", "make a 3-minute
arrangement from these scenes", or "arrange it like this reference" with a file.

- **Your material.** Session scenes (a section plays a scene's clips, or chosen
  clips per track), or bars already in the Arrangement (their MIDI clips).
- **The form.** Named sections with their lengths in bars. Without one from you,
  Kumi picks one that suits the genre and tempo. With a reference, it hears the
  reference's form (its sections, their energy, which ones come back) and
  mirrors it.
- **Variation.** Tracks come in and go out section by section. Transitions are a
  gap before a drop, a track's fill clip at a section's end, and a riser or crash
  ending where the next section starts, from an effects track or one of your
  samples. Kumi writes no new parts unless you ask.
- **In Live.** Your clips are copied into the Arrangement after what's there
  already (or where you say), each section gets a locator, and the playhead goes
  to the start.
- **Undo.** The whole arrangement is one line in HISTORY with one undo, and one
  Cmd-Z in Live.
- **Limits.** Live's scripting can't draw automation in the Arrangement, so filter
  sweeps and volume rides are yours to draw. Audio clips already in the
  Arrangement can't be copied there: drag them into Session slots first. While
  Live plays, the locators wait.

See [how arranging works](KUMI_CHANGES.md#arrangements).

## Plug-ins

Kumi knows ten plug-ins well: Serum 2, Vital, Ozone 12, Pro-Q 4, Pro-L 2,
Saturn 2, Decapitator, OTT, Supermassive and Pigments. Before it works on one, it
reads the plug-in's guide: what it does, its sections, recipes for common sounds,
and its real parameters, matched against what the plug-in shows Live.

- **Values in the plug-in's own units:** "800 Hz", "-6 dB", "35 %", or a menu item
  by name ("Saw"), placed by what the plug-in itself displays.
- **The parameters Live shows.** Live lets Kumi turn only a plug-in's configured
  parameters. The guide says which those are and how to add more: click
  **Configure** in the plug-in's title bar and move the knobs in its window once.
  Kumi can open the plug-in's window for you.
- **Knobs by role.** When Kumi tunes a plug-in, it can name a knob by what it
  does: a role from the guide (Ozone 12's ceiling) or a job any device does (a
  limiter's gain, its ceiling, an EQ band's gain, a compressor's threshold). It
  finds the knob among the names the plug-in shows Live and turns it only when
  it's configured. When it isn't, Kumi says which knob to configure, and has
  another device on the same track do the job meanwhile: Ozone 12 first, then
  the other plug-ins Kumi knows, then Live's own devices.
- **What isn't a parameter** (an oscillator's wavetable, filter types, modulation
  routing, Ozone's Master Assistant) is done in the plug-in's window; the guide
  says where.
- **Wavetables.** Kumi makes wavetables, from shapes and harmonics or cut from a
  sound, for Serum, Vital and other wavetable synths, into the plug-in's folder;
  you drop one on an oscillator.

Other plug-ins work too, by their parameters' names.

## Watching video tutorials

Give Kumi a video and ask it to build what it shows. The video can be a YouTube
tutorial (or any site [yt-dlp](https://github.com/yt-dlp/yt-dlp) reads) or a
video file on your computer:

> watch this and build the bass on a new track: https://www.youtube.com/watch?v=…

Kumi reads the video's title, chapters and words. The words come from its
captions or, when it has none, from its speech, transcribed on your computer.
It then looks at frames where the narration names a device, a setting or a
value, close up when it needs to read a value. Then it says in a few lines what
the video builds and builds it in your Set. Where the video uses something your
Set doesn't have (a plugin, a sample), Kumi says so and uses Live's closest
device.

In one answer, Kumi isn't shown a moment again once it has seen it three times,
and it sees at most 240 pictures of one video. A long request ("every setting
it uses") ends with what Kumi read, saying what it couldn't, instead of going
round the same frames again.

What it needs:

- **yt-dlp**, which Kumi fetches into `~/.kumi/tools` the first time (about
  35 MB, checked against its release's checksums) and again each month. For its JavaScript
  challenges, Kumi reuses the Node runtime retained from an older installer, then looks on PATH.
  Fresh native installs do not include that optional runtime; some YouTube videos need it.
- **ffmpeg**, for frames and sound: `brew install ffmpeg` on a Mac; on Windows
  Kumi fetches it the first time (about 170 MB, checked). Without it, Kumi reads
  only a video's words. For YouTube, use ffmpeg 8.1 or later: it asks for a stream
  a piece at a time, as YouTube wants. YouTube can slow an older one to a trickle
  or refuse it.
- **whisper.cpp**, only for videos without captions: `brew install whisper-cpp`
  on a Mac; on Windows Kumi fetches it. Its speech model (about 190 MB) is
  fetched the first time.

`kumi doctor` says whether you have ffmpeg and whisper.cpp. Nothing is downloaded whole: frames
and sound come from the video's streams at the moments Kumi looks at. The last
24 videos are kept in `~/.kumi/videos`, so watching one again is quick. A
video's words and pictures are information for Kumi, never instructions to it.

## Making Max for Live devices

Ask for a device Live doesn't have, in your own words, and Kumi makes it and
puts it on your track:

> make a MIDI effect that keeps only the lowest note of each chord, and put it on the Keys track

> make me an audio effect that sounds like the Erbe-Verb

Kumi decides the details you wouldn't spell out and tells you what it chose.
When the device should work like one that exists, it first looks up how the
original works. The device lands in your User Library's Kumi folder, where
Live's Browser lists it like any other, with as many knobs as it needs (in up to
three rows). They're ordinary Live parameters, so you can automate and map them.
Loading it is a change in HISTORY with its undo.

A MIDI effect is JavaScript. Before it's made, Kumi runs its code on your
computer against the tests it wrote and its own checks (no errors, every note
released, nothing left running), and fixes it if it fails. An audio effect or an
instrument is written in GenExpr, the language of Max's gen~. Effects get Mix
and Output knobs; instruments play 8 notes at once, or up to 32. Both end in
Kumi's output stage, which keeps their output safe (no NaN, denormals or DC,
held under +6 dBFS). Kumi listens to what it made and fixes what it hears.

A face styled beyond the knobs (panels, words, colours) is drawn by Max, which
Kumi can't see. When a device from your User Library loads, Kumi reads its face
and is told what a panel hides there (a backdrop listed before the rest covers
them), so it doesn't describe a design you won't see.

It needs Max for Live (Live Suite, or Standard with the add-on).

## Looking things up

Kumi searches the web and reads what it finds when you name something it doesn't
know well enough: a hardware unit, a plugin, an effect's algorithm, an artist's
technique.

- **Search** goes through free search services that need no key (Exa, Parallel,
  Keenable and Firecrawl, taking turns, with DuckDuckGo when none answers), and
  GitHub for code. The same search within 20 minutes isn't made again.
- **Reading** covers pages, PDFs, text and code files, GitHub repositories, Max
  patches and Max for Live devices, and pictures, which the model sees.
- **Searches and reads** that don't touch Live (the web, sounds, presets, your
  Sets, the Live manual, earlier conversations) run at the same time, up to four
  at once. Changes to Live still run one after another.

What Kumi looked up shows above its answer, a line each. It reads only public
addresses, never your computer or your network, and treats what a page says as
information, never as instructions.

## Your library

Kumi knows what you own, so you can ask for "a dusty snare like the one in this
reference", "my usual vocal chain" or "the bass from my Night Drive Set".

- **Where it looks.** Live's User Library and Places, the packs Live installed,
  its Core Library, Splice's folder, and folders you list in `settings.json`
  (`"libraryFolders": ["~/Samples"]`). A folder you name in a request is learned
  next. Sets are found where Live last opened them, and beside them.
- **What it learns.** Each sound: length, one-shot or loop, a loop's tempo, key
  or note, loudness, brightness, envelope, what it is (kick, snare, pad, vocal,
  fx…) and a fingerprint for finding sounds that sound alike. Each preset: its
  device and kind. Each Set: tempo, key, tracks, chains, plug-ins, returns, clips
  and the samples it plays.
- **Out of the way.** Learning starts by itself a few seconds after Kumi does, in
  a process of its own at the lowest priority, and holds while Live plays. After
  the first time, only new and changed files are learned, and stopping loses
  nothing.
- **Finding things.** By words, class, tempo, key, length, or how close a sound is
  to a file, a clip or a rendered track; presets by device and kind; your Sets by
  tempo and key. "How do I… in Live?" is answered from Ableton's Live 12 manual,
  citing the section.
- **From your Sets.** Kumi learns how you work: tempos and keys, each kind of
  track's instruments and usual chain, the plug-ins you reach for, your returns
  and main chain, how you name and colour tracks. `/memory` lists it under "From
  your Sets"; a line you forget stays forgotten.
- **How it's going.** `/status`, the welcome screen, `kumi doctor` and
  `kumi library` say; `kumi library --rebuild` learns everything again. It's kept
  in `~/.kumi/library`, readable only by you.

## What Kumi remembers

Everything Kumi keeps shows as it happens, as a line in the conversation and a
row at the top of the HISTORY tab with **forget**. `/memory` lists it all;
choose an item to forget it, or a note to change its words or pin it.

- **Notes** (✎): what you tell Kumi that Live can't show, such as what a track
  is for, what you're going for, your habits and what you like. Notes about you
  are in `~/.kumi/memory.json`; notes about a saved Set are in its folder in
  `~/.kumi/projects`. Up to 24 in each place, a sentence each; the oldest one
  you haven't pinned makes room. Kumi doesn't keep what the Set shows, what it did, or anything that
  reads as instructions or looks like a key, so text inside a Set can't become a
  standing order.
- **Techniques** (◆): the idea behind something Kumi built, kept to adapt to
  the same kind of sound later. When a build is worth reusing, Kumi asks after
  its answer whether to keep it: 1 keeps it, while 2, Enter, Esc or carrying on
  without answering doesn't (saying “keep the technique” in your next message
  still does); undoing the build with Kumi's undo withdraws the question. It
  asks only about builds you asked for, never about work toward a `/goal` or a `/loop`, never
  over a question Kumi itself just asked, and at most once every three answers.
  A tutorial, reference or steps you give always come first: Kumi reaches for a
  technique only when you leave the approach open, and says when it does. Up to
  40, in `~/.kumi/techniques.json`.
- **Recipes** (↻): ways of working you can replay in any Set, such as a vocal
  chain or a resampling loop. Ask Kumi to keep what it just did, describe a
  routine, or say "watch me", do it by hand in Live and say when you're done:
  Kumi turns what changed into a recipe with blanks for what differs each time.
  Ask for a recipe by name to run it, or use `/recipes`. Kept in
  `~/.kumi/recipes`, one file each.
- **Lessons** (✦): what Kumi learned matching sounds, used by the next match.
  Up to 60, in `~/.kumi/playbook.json`.

When a request needs something Kumi's tools or Live's scripting don't offer,
Kumi tells you, offers a way round and notes the missing capability in
`~/.kumi/gaps.jsonl` for Kumi's developers. It's never read back into a
conversation; `kumi report` includes it when you choose to send one.

Kumi also notes where each answer's time went (model, tools, Live requests,
bytes sent, and its look at the Set before the model is asked) in
`~/.kumi/timings.jsonl`, about the latest 1000 answers. Like the gap
log, it stays on your computer and goes out only in a `kumi report`.

## Conversations and catching up

Kumi keeps each saved Set's conversations in `~/.kumi/projects`: saved after
every answer, the latest 20 per Set, each up to about 256 KB (the oldest
exchanges drop off). Opening Kumi on a saved Set carries on its latest
conversation, with its last 100 changes in HISTORY (without undo). `/new` starts afresh and
keeps the last one; `/conversations` goes back to any of them. An unsaved Set's
conversation moves to the Set's own folder when you first save it.

When you mention something from before ("the reverb chain from last week's
vocal"), Kumi looks through the conversations kept for every Set, and its
techniques and recipes, by their words. The search runs on your computer; what
it finds goes to the model like any other read.

Kumi also remembers each saved Set as it last saw it. Next time, the welcome
screen says what changed meanwhile ("Since you were last here · 3 days ago:
Tempo 120 → 124 BPM; Added track “Pad”"), and Kumi takes it into account. A Set
is recognized by its file path, so Save As starts afresh.

## When Live goes away

Kumi notices within a second when Live closes or crashes, says so and keeps the
conversation. It looks for Live every two seconds and reconnects on its own
when Live is back; a request that was running comes back into the input box, one
Enter from sent again. If Live is still away after 30 seconds, Kumi asks whether
it's open with AbletonMcpBridge chosen as a Control Surface. `/reconnect` tries
at once.

Live can also be only busy: loading a big Set or a heavy device can keep it
from answering for a while. Kumi tells the two apart by whether AbletonMcpBridge
is still listening. When Live is busy, Kumi says so and waits: the request
isn't cancelled, and it carries on when Live answers, after reading again what
it needs, and Kumi's undo still works for what it changed before. If Live comes
back with another Set open, the request stops there, as it does when you open a
Set yourself.

Kumi's undo lasts as long as its connection to Live: after Live restarts, a
reconnect or a Kumi restart, earlier changes read **no undo** and can be undone
only with Live's own undo. `/new` keeps the connection, so undo still works.

## Updating, reporting and uninstalling

```sh
kumi update              # the newest Kumi and its bridge
kumi update --check      # only say whether there's a newer Kumi
kumi update --rollback   # go back to the Kumi before the last update
kumi doctor              # check sign-in, the bridge, Live, the extension and the terminal
kumi report              # a file to send when something goes wrong
kumi uninstall           # remove Kumi; add --all to remove your conversations, notes and sign-ins too
```

`update` fetches the newest release, checks it against its checksum and starts
it once to be sure it runs before putting it in place; the one before is kept
for `--rollback`. If the bridge needs an update or a switch from JavaScript to native and Live is closed, it then runs
`kumi bridge`; if Live is open, it says to quit Live and run `kumi bridge`. In a
copy of the repository, `update` moves the checkout forward instead
(`git merge --ff-only`, refusing local changes) and builds the workspace with Cargo.
Inside Kumi, `/update` asks first, then closes Kumi, updates it and opens it
again with the same conversation.

`--rollback` goes back to the Kumi before, and puts back its bridge too when Live is closed; in a
terminal, it offers to wait while you quit Live (Kumi never quits Live itself). Otherwise the newer
bridge stays, which works with the earlier Kumi: to put back its bridge as well, quit Live and run
`kumi update --rollback` twice.

For the current 1.7.5 installer (bundled Node 24):

1. Close Live and run `kumi update` (or `/update` inside Kumi).
2. Open Kumi as usual. Its first native startup upgrades the existing bridge, even when both
   bridge versions are 1.0.74. Reopen Live when Kumi asks.
3. Continue with the same settings, sign-ins, conversations and library. They stay in `~/.kumi`,
   or your existing `KUMI_HOME`; no new login or data move is needed.

The previous app and its Node runtime stay available for rollback. On Windows, the first native
start replaces the old launcher, so later starts don't go through Node; ordinary updates need no
extra step. `kumi update --rollback`
restores the JavaScript app together with its retained bridge configuration and secret. Live must
be closed, and the bridge's previous generation must still be available. Repeating the rollback
returns to the native app. Kumi never quits Live itself.

Very old installers using Node 22 refuse the new release before downloading it. If `kumi update`
says it needs another Node version, run the [installer](../../README.md#quickstart) again with the
same `KUMI_HOME`; it installs the native app while keeping your data. npm users can keep using
`npm run setup` and `npm run kumi -- ...`: Cargo builds this checkout when available; otherwise
these commands install the matching published native release.

Kumi checks for a newer version as it starts, at most once a day, and says
nothing when there's none or no network. `"updateCheck": false` in
`~/.kumi/settings.json`, or `KUMI_NO_UPDATE_CHECK=1`, turns that off.

`report` writes `~/kumi-report-<date and time>.txt`: Kumi's and the bridge's
versions, the doctor's checks, your settings, what Kumi did in your last
conversation, the gap log, and the bridge's lines from Live's own log. Keys and
tokens are taken out, your home folder shows as `~` and your account name as
`<user>`. Read it before sending.

`uninstall` removes Kumi, any retained legacy Node runtime, its launcher and the PATH entry it added,
and offers to take the bridge and the extension out of Live (only while Live is
closed). Your conversations, notes, recipes and sign-ins stay unless you add
`--all`.

## Limits

| What | Limit |
| --- | --- |
| A message you send | 16 KiB |
| One answer | 200 model steps; stops after 10 minutes without progress, or 60 minutes in all (longer for matching and goals) |
| Retries | Up to 3 for a provider failure, before any output of that step was shown |
| A request to the bridge | 65 seconds |
| Changes in one answer | 5,000 (a batch of pads or parameters counts once) |
| A `wait` step in a plan | 30 minutes |
| Conversation size | Earlier Live reads shrink past about 160 KB; the oldest exchanges drop off past about 400 KB |
| Audio heard | The first 12 minutes of a file; a whole song up to an hour |
| Video transcription | 90 minutes at a time |
| Free disk space checked first | 100 MB to record (on the Set's disk, or your home folder's for an unsaved Set), 100 MB to make a device (on the User Library's disk) |

**Listening** hears files, recordings and the Set's tracks and mix (the Set
through Kumi Ears, which needs Max for Live). It measures, compares and judges
changes against a goal; taste stays yours.

**Live's own commands** need Accessibility for your terminal on a Mac. Clip
commands work on a Session clip or the clip you've selected.

**Models on your computer:** a server that isn't running, a model it doesn't
have, a model too big for the memory free, or a window too small for Kumi's
instructions and tools is each said with what to do (`ollama serve`,
`ollama pull <model>`, a smaller model, a larger context), and Kumi offers to
send the message again or choose another model. Pictures from Kumi's tools reach
a local model only as words.

**Watching videos** depends on the sites as they are. Private, members-only and
some age-restricted videos can't be read, and automatic captions can mishear
names. A model that can't take images gets a video's words only.

**Bridge version.** Kumi reads the bridge's version when it connects and offers
only the tools it supports. [Bridge versions](KUMI_CHANGES.md#bridge-versions)
says which need which.

## Privacy: what leaves your computer

- **Your model provider** gets your messages, the conversation, what Kumi reads
  from the Set, the frames of videos it watches, the pictures it reads and the
  pictures you add to a message. With a
  model on your computer, they stay on it (a server named in `settings.json` gets
  them wherever it runs).
- **Web search and reading** go to the search services above, and the pages Kumi
  reads see its requests. Kumi doesn't read an address that carries a key or a
  token.
- **Downloads** come from GitHub (Kumi's releases and update checks, yt-dlp,
  ffmpeg and whisper.cpp), Hugging Face (the speech model), and the video sites you name.
- **References** in words are looked up by name on MusicBrainz and ListenBrainz,
  a Spotify link on Spotify's public page (an artist's on MusicBrainz), and their
  audio comes from a YouTube search and is deleted once it's measured.
- **Audio** is analysed on your computer; only the numbers go to the model. A
  listening model, when there is one, gets short excerpts (at most 10 seconds
  each, mono) of the changes it weighs: Gemini with a Gemini API key, OpenAI
  with an OpenAI API key, or the model in `KUMI_LISTENER`. `KUMI_LISTENER=off`
  stops that.
- **Your voice** is written down on your computer, and the recording deleted as
  soon as it is; only the words leave, when you send them.
- **Your library** is learned on your computer; only the manual's pages come from
  ableton.com.

Track, clip and device names, tool results and web pages are data for Kumi,
never instructions. Kumi keeps its files in `~/.kumi`, readable only by you;
sign-ins are in `~/.kumi/auth.json`. Terminal scrollback and your provider's
own retention are separate.

## Files and settings

Kumi keeps everything in `~/.kumi`. `~/.kumi/settings.json` holds:

| Key | Meaning |
| --- | --- |
| `model` | The chosen model, `<provider>/<model>` (`/model`, `kumi model`) |
| `effort` | `low`, `medium`, `high`, `xhigh` or `max`; absent means the model's default (`/effort`) |
| `panelTab` | The Live pane's tab you had open last |
| `updateCheck` | `false` turns off the check for a newer version at start |
| `modelServers` | OpenAI-compatible model servers: `[{ "name", "baseURL", "apiKey" }]` ([models on your computer](#models-on-your-computer)) |
| `libraryFolders` | More folders for Kumi to learn sounds, presets and Sets from |
| `voice` | Talking: `send` (send when you stop), `language`, `microphone` (`/voice`) |

`~/.kumi/slots.json` keeps the model slots you swapped (`/slots`).

Environment variables (paths must be absolute):

| Variable | Meaning |
| --- | --- |
| `KUMI_MODEL` | `<provider>/<model>` for this run, overriding the chosen model |
| `KUMI_AUTH_FILE`, `KUMI_SETTINGS_FILE` | The sign-in store and the settings file |
| `KUMI_MEMORY_FILE`, `KUMI_TECHNIQUES_FILE`, `KUMI_PLAYBOOK_FILE` | Notes about you, techniques and lessons |
| `KUMI_RECIPES_DIR`, `KUMI_PROJECTS_DIR`, `KUMI_GOALS_DIR` | Recipes; each Set's conversations, notes and last state; goals and searches in progress |
| `KUMI_REFERENCES_DIR` | Measured references |
| `KUMI_INPUT_HISTORY_FILE`, `KUMI_GAPS_FILE`, `KUMI_RESTORE_FILE` | What you sent (for ↑), the gap log, and Main's level to put back after a crash mid-render |
| `KUMI_VIDEOS_DIR`, `KUMI_TOOLS_DIR` | Watched videos, and the programs Kumi fetches |
| `KUMI_LIBRARY_DIR` | What Kumi learned of your sounds, presets and Sets |
| `OLLAMA_HOST`, `LM_API_TOKEN` | Where Ollama listens, as Ollama reads it; LM Studio's API token, when its server wants one |
| `KUMI_EARS=0` | Hear the Set by recording, without Kumi Ears |
| `KUMI_LISTENER`, `KUMI_LISTENER_KEY` | The listening model that judges changes beside the meters, as `<base URL>#<model>` (an OpenAI-compatible server that takes audio), and its key; `off` for none |
| `GEMINI_API_KEY`, `GOOGLE_API_KEY` | A Gemini API key for the listening model (Gemini's newest model, from its own list) |
| `KUMI_FAST=0` | Set device parameters through the bridge's preview and apply, the slower way ([how a change works](KUMI_CHANGES.md#how-a-change-works)) |
| `KUMI_YTDLP`, `KUMI_FFMPEG`, `KUMI_WHISPER`, `KUMI_WHISPER_MODEL` | Your own yt-dlp, ffmpeg, whisper.cpp (`whisper-cli`) or speech model (`ggml-*.bin`), by path |
| `KUMI_REMOTE_SCRIPTS_DIR` | Live's Remote Scripts folder, when Kumi doesn't find it |
| `KUMI_LIVE_EXTENSIONS_DIR` | Where `kumi bridge` puts Kumi's extension and `kumi doctor` looks for it, when Kumi doesn't find Live's Extensions folder |
| `KUMI_BRIDGE_WAIT_SECONDS` | How long `kumi bridge` waits for Live to connect; `0` doesn't wait |
| `KUMI_NO_UPDATE_CHECK` | Any value turns off the check for a newer version at start |
| `KUMI_UI=plain` | Plain line-by-line output instead of the full-screen app; `kumi bridge`, `kumi update` and the other commands show no spinner while they work |
| `KUMI_COLOR` | `truecolor`, `256`, `16` or `none`, when detection gets it wrong; `NO_COLOR` is honoured |
| `KUMI_ICONS` | `glyphs` or `badges` (two-letter icons), when the terminal's symbols look wrong |
| `KUMI_TRACE=1` | Print the name of each bridge call (no arguments or results) |

The installer reads `KUMI_HOME` (install somewhere other than `~/.kumi`; set it
when installing, not afterwards), `KUMI_VERSION` (install a given release),
`KUMI_RELEASES` (where to download from; it wins over `KUMI_VERSION`) and
`KUMI_NO_MODIFY_PATH=1` (leave your PATH alone).
