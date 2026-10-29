# Kumi guide

English · [简体中文](../zh-CN/KUMI_GUIDE.md) · [日本語](../ja/KUMI_GUIDE.md)

Kumi is a producer agent for the Ableton Live Set you have open, in your
terminal. It reads the Set, makes the changes you ask for and shows each one in
HISTORY. It plays, records and renders, listens to audio and compares it with a
reference, watches video tutorials, makes Max for Live devices, looks things up
on the web, and remembers how you work. It works in REAPER projects and Renoise
songs too: most of this guide speaks of Live, and
[Connect to REAPER or Renoise](#connect-to-reaper-or-renoise) says what differs
there.

To install Kumi and connect it to Live, follow [Quickstart](../../README.md#quickstart).
This guide covers everything after that. For the keys and the screen, see
[commands, keys and screens](KUMI_TUI.md); for how changes and undo work, see
[how Kumi changes your Set](KUMI_CHANGES.md).

## Sign in and choose a model

Inside Kumi, `/login` signs in (the first time, Kumi's setup asks first): to
ChatGPT with your plan (the browser opens and
the sign-in comes back on `localhost:1455`), or to any other provider below
with an API key, pasted into a box that shows only dots. Kumi checks a key with
its provider before keeping it; if the provider can't be reached, it keeps the
key and says it isn't checked yet. `/logout` signs out.

In setup's sign-in step:

- Kumi first checks it can reach ChatGPT's sign-in. When it can't, it says why
  (DNS, TLS, a proxy, no answer in time, or no connection) and names the proxy
  settings it went through, or says that `HTTPS_PROXY` sets one.
- **d** signs in to ChatGPT with a code instead of the browser: enter it in a
  browser on any device. Over SSH, Kumi uses the code by itself, in `/login`
  and `kumi login` too; **b** (or `--browser`) goes back to the browser's link.
- The browser's link shows whole, and **c** copies it.
- While Ollama or LM Studio runs, **On this computer** is a choice too.

**Providers.** Sign in to any of these with `/login` or `kumi login <id>`; `kumi auth` shows
which you're signed in to. A key variable works too, and a key saved with
`/login` comes first.

| Provider | Model names | Key variable | Where to get a key |
| --- | --- | --- | --- |
| ChatGPT plan | `openai-codex/<model>` | none: `/login` opens the browser (**d** for a code), or `kumi login openai-codex` (`--device` without a browser; over SSH a code by itself, `--browser` for the browser) | your ChatGPT plan |
| Anthropic API | `anthropic/<model>` | `ANTHROPIC_API_KEY` | platform.claude.com (API keys) |
| OpenAI API | `openai/<model>` | `OPENAI_API_KEY` | platform.openai.com/api-keys |
| OpenCode Zen and Go | `opencode/<model>`, `opencode-go/<model>` | `OPENCODE_API_KEY` (one for both) | opencode.ai/auth |
| OpenRouter | `openrouter/<model>` | `OPENROUTER_API_KEY` | openrouter.ai/settings/keys |
| Venice | `venice/<model>` | `VENICE_API_KEY` | venice.ai/settings/api |
| Google Gemini | `gemini/<model>` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` (the listening model's key too) | aistudio.google.com/apikey |
| DeepSeek | `deepseek/<model>` | `DEEPSEEK_API_KEY` | platform.deepseek.com/api_keys |
| xAI | `xai/<model>` | `XAI_API_KEY` | console.x.ai |
| Z.ai (GLM) | `zai/<model>` | `ZAI_API_KEY`, `ZHIPU_API_KEY` | z.ai/manage-apikey/apikey-list |
| Moonshot (Kimi) | `moonshot/<model>` | `MOONSHOT_API_KEY` | platform.kimi.ai/console/api-keys |
| Alibaba Cloud (Qwen) | `alibaba/<model>` | `DASHSCOPE_API_KEY` | modelstudio.console.alibabacloud.com (API keys) |
| MiniMax | `minimax/<model>` | `MINIMAX_API_KEY` | platform.minimax.io (interface key) |
| Xiaomi MiMo | `xiaomi/<model>` | `MIMO_API_KEY`, `XIAOMI_API_KEY` | platform.xiaomimimo.com (API keys) |
| StepFun | `stepfun/<model>` | `STEPFUN_API_KEY`, `STEP_API_KEY` | platform.stepfun.ai/interface-key |
| Tencent TokenHub | `tencent/<model>` | `TENCENT_TOKENHUB_API_KEY` | console.tencentcloud.com/tokenhub/apikey |
| Groq | `groq/<model>` | `GROQ_API_KEY` | console.groq.com/keys |
| Cerebras | `cerebras/<model>` | `CEREBRAS_API_KEY` | cloud.cerebras.ai (API keys) |
| Fireworks AI | `fireworks/<model>` | `FIREWORKS_API_KEY` | app.fireworks.ai/settings/users/api-keys |
| Together AI | `together/<model>` | `TOGETHER_API_KEY` | api.together.ai/settings/api-keys |
| DeepInfra | `deepinfra/<model>` | `DEEPINFRA_API_KEY` | deepinfra.com/dash/api_keys |
| Mistral | `mistral/<model>` | `MISTRAL_API_KEY` | console.mistral.ai (API keys) |
| Ollama Cloud | `ollama-cloud/<model>` | `OLLAMA_API_KEY` | ollama.com/settings/keys |
| NVIDIA | `nvidia/<model>` | `NVIDIA_API_KEY` | build.nvidia.com (API keys) |
| Hugging Face | `huggingface/<model>` | `HF_TOKEN` | huggingface.co/settings/tokens |
| Ollama, on your computer | `ollama/<model>` | none: found while it runs | — |
| LM Studio, on your computer | `lmstudio/<model>` | none: found while its server runs | — |
| Another server | `<name>/<model>` | its `apiKey` in `settings.json` ([below](#models-on-your-computer)) | — |

- A model name keeps its provider's `/` (`openrouter/moonshotai/kimi-k3.1-pro`).
- Use a pay-as-you-go key with Z.ai and StepFun: their coding plans are limited to the tools they list.
- NVIDIA's free trial keys are for testing and evaluation only; some of its models answer late and don't work with Kumi yet.
- Hugging Face needs a fine-grained token with the Inference Providers permission.
- Google's Gemini API terms are for professional use, and in the EEA, Switzerland and the UK need its paid tier.
- `/model` lists ChatGPT, Anthropic, OpenAI and OpenCode whether you're signed in or not, and the others once you are; its **More providers** opens `/login`.
- A custom server named like a provider Kumi added later keeps its name's id: Kumi hides that provider for you, and `kumi doctor` says how to switch.

A key saved with `/login` is used first; without one, Kumi uses the key in the
environment, and can't sign out of that (unset it instead). Paste an API key
only into Kumi's key box or its `kumi login` prompt, never into a message.
Claude and Gemini subscription sign-ins aren't offered, because their providers
don't permit them in third-party tools. OpenCode's Gemini models aren't
supported yet.

`/model` lists each provider's models, read from the provider itself, so a model
released today is there without a Kumi update; typing narrows the list. With no
model chosen, Kumi uses the first model of the first provider you're signed in
to (in the order of the table) and says which. `/effort` sets how hard the model
thinks, from the levels that model offers (`none` to `max`); lower answers
sooner. Both apply from your next message and are kept for next time. Changing
model keeps the conversation, except the earlier model's private reasoning,
which belongs to that model.

What Kumi knows of each model (its effort levels, whether it can use tools, what
it takes beside words) comes from its provider's list first, then from
[models.dev](https://models.dev)'s catalog, then from its name:

- Models that don't chat (pictures, embeddings, speech) aren't listed.
- **chat only**: the model can't use tools. Kumi talks with it about your Set but
  can't change anything, as with a model on your computer that can't. A model
  that turns tools down when asked is treated so until Kumi restarts.
- **unverified**: neither its provider nor models.dev describes it, so Kumi goes
  by its name.
- **deprecated**: its provider is retiring it.

Kumi carries a copy of the catalog and fetches a newer one at most once a day;
`"modelCatalog": false` in `settings.json` turns that off.

When an answer fails because a sign-in is missing or was refused, Kumi offers to
sign in and then sends your message again. When the provider doesn't offer the
chosen model, Kumi offers to choose another. When the provider's own list no
longer shows your saved model (one retired from a plan's sign-in, say), Kumi says
so when it starts and opens `/model`. When an answer breaks off partway
(a dropped connection, say), Kumi carries on once from where it stopped and says
so in the status line; if it breaks off again, Kumi offers to send your message
again.

### Limits, outages and a fallback model

- **A plan's limit** (ChatGPT's five-hour or weekly window): Kumi waits for it
  to reset and carries on with the same answer. NOW says when ("carrying on at
  14:20"), the answer's hour doesn't count the wait, the computer stays awake,
  and Esc stops it. A limit more than 6 hours away stops the answer instead,
  saying when it resets and offering a fallback, another model or another
  sign-in.
- **A fallback model** carries on instead of waiting: in `/model`, ctrl+f makes
  the selected model your fallback (ctrl+f on it again clears it). It takes
  over at the next step when your model's plan limit is reached, its credit is
  used up, or its provider is still down after the retries. The header names it
  while it answers; your model is back at your first message after the reset.
  A fallback you aren't signed in to is left out, and Kumi says so once.
- **A busy or failing provider:** Kumi waits as long as it asks (Retry-After,
  however long, in seconds or as a date) and tries a step up to 3 times.
- **No connection:** Kumi keeps trying, waiting a little longer each time (up
  to 30 s), for as long as the answer may run.
- **Plan usage:** from 80% of a window, the header shows "plan 84% · resets
  14:20".

### Models on your computer

- **Ollama** and **LM Studio** are found while they run, with no sign-in.
  `/model` lists each as its own provider ("Ollama · on this computer") with the
  models it has; one that's installed but closed says how to start it. Signed in
  nowhere, Kumi starts with one of their models that can change the Set,
  preferring one already loaded.
- **Another server** (llama.cpp's `llama-server`, vLLM, Jan, a gateway…) goes in
  `~/.kumi/settings.json`, on this computer or elsewhere. Its name becomes its id
  (`llama.cpp` is `llama-cpp/<model>`). `format` says how it takes a request:
  `chat-completions` (OpenAI's Chat Completions, without `format`), `responses`
  (OpenAI's Responses) or `anthropic` (Anthropic's Messages). What Kumi knows of
  its models comes from its own list, so they're marked unverified:

  ```json
  { "modelServers": [
    { "name": "llama.cpp", "baseURL": "http://127.0.0.1:8080/v1" },
    { "name": "Studio PC", "baseURL": "http://192.168.1.20:8000/v1", "apiKey": "…" },
    { "name": "Gateway", "baseURL": "https://llm.example.com/v1", "apiKey": "…", "format": "anthropic" }
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
2. **Connect to Live**: Kumi puts the bridge in place and opens Live.
   - **Live isn't open:** Kumi first looks for Live 12. If it isn't installed,
     or only an older Live is, Kumi says so; **Look again** checks once you've
     installed it. If Live 12 was never opened, Kumi opens it once, so it makes
     its User Library.
   - **Live is open:** choose **Restart Live now** (Kumi asks Live to quit, and
     Live asks you to save your work first) or **I'll quit it** (Kumi waits while
     you quit Live). If Live is still open a while after Kumi asked (you chose
     Cancel when it asked about saving), Kumi says so and offers to ask again.
   - **On a Mac,** macOS may ask the first time whether your terminal can
     control Live: choose OK. If you said no, Kumi waits while you quit Live,
     and says where to allow it: System Settings › Privacy & Security ›
     Automation.
   - Once Kumi has Live closed, Live opens again however the step ends, with
     the Set it had open when Kumi knows it (otherwise your Set is in File ›
     Open Recent Set).
3. **Control Surface**: the first time, open **Settings → Link, Tempo & MIDI** in
   Live and choose **AbletonMcpBridge** as a Control Surface. Kumi notices by
   itself, and Live remembers the choice. After about 45 seconds, Kumi adds where
   Settings is (the Live menu on a Mac, Options on Windows), and that a dialog in
   Live comes first.

On Linux, where Live doesn't run, setup only signs in.

Esc on a step leaves it for later: Kumi chats without Live for now. Sign in and
Connect to Live are offered again next time; past Connect to Live, Kumi connects
by itself whenever Live answers. Quitting Kumi while the bridge goes in waits
until it's in place.
`kumi bridge` does the same from a shell, with Live closed: it asks you to
confirm (`--yes` confirms beforehand, and without a terminal nothing changes
without it), refuses while Live is running, and never quits or starts Live. Both install or update the bridge through the bridge's own
lifecycle, with checks, receipts and rollback, and keep its settings and secret
across updates. `kumi bridge` waits up to ten minutes for Live to connect, and
says when it does. `kumi update` runs it for you (with `--yes`, and without the
wait) when the bridge in Live is older than Kumi's and Live is closed. Otherwise
`kumi update` and `kumi doctor` say to run `kumi`, which puts the bridge in Live
and can restart Live for you.

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
--inference-only` chats without Live, and keeps the conversation only when it
carries one on (`-c` or `-r`).

**Willington.** Willington's native bindings come in Kumi's bridge from the
release that carries their files; until then, you can install Willington
yourself. Either way they're off until you turn them on with `/willington`.
Then Kumi can also map rack macros, set
chain zones and, with a passing self-test, edit Follow Actions, on the Live versions Willington has
bindings for: Live 12.4.15b4 and b5 on macOS ARM64 (chain zones on b5 only) and
Live 12.4.15b5 on Windows x64. See [Willington](WILLINGTON_INTEGRATION.md).

## Connect to REAPER or Renoise

Kumi works in REAPER 7 or later and Renoise 3.5 or later, on macOS, Windows and
Linux. Each has a bridge of Kumi's own: an extension REAPER loads from its
`UserPlugins` folder (`reaper_kumi`), and a tool Renoise loads from
`Scripts/Tools` in its preferences (`com.kumi.Bridge.xrnx`). Kumi reaches it
through its host (`daw-mcp-server`, installed beside Kumi), on your computer
only: the bridge listens on 127.0.0.1 and answers only Kumi, with a secret kept
in `~/.kumi`. There's no Control Surface to choose.

**Setting up.** Run `kumi` and open REAPER or Renoise, before or after Kumi:
Kumi works with the DAW that's open. The first time, setup signs you in, then
runs **Connect to REAPER** (or **Connect to Renoise**), on Linux too: Kumi puts
its extension (or tool) in place and opens the DAW.

- **The DAW is open:** it loads Kumi's bridge only as it starts, and Kumi never
  quits it for you. Choose **I'll quit it** and quit it yourself (it asks to
  save first); Kumi puts the bridge in and opens it again. A project or song
  you saved is in the DAW's list of recent ones.
- **It isn't installed:** Kumi says where to get it (reaper.fm, renoise.com);
  **Look again** checks once it's there. An older REAPER or Renoise that's open
  is named, and stays open.
- **It was never opened:** Kumi opens it once, so it makes its own folders;
  quit it when Kumi asks, and Kumi opens it again with the bridge.
- **Esc** leaves the step for later, as with Live: Kumi chats without the DAW.

`kumi --daw reaper bridge` (or `renoise`) does the same from a shell: it asks
before writing into the DAW's folder (`--yes` answers beforehand), puts the
bridge in or brings it up to date, and says to restart the DAW if it's open.

**Which DAW.** `kumi` works with the DAW that's open, its bridge in yet or not.
With several open, it takes the one whose bridge is in, else the one you used
last, else Live, and says when another is open too ("Renoise is open too: Kumi
works with REAPER. /daw renoise switches to Renoise."). With none open, it takes
the one whose bridge is in (with several, the one you used last) and says to
open one of yours. Open another DAW while Kumi isn't connected to its own (or
its own doesn't have Kumi's bridge yet), and Kumi follows: it says so, closes
and opens again with that one, once you're not typing or waiting for an answer.
`/daw` says which DAW Kumi works with, and `/daw <name>` (`live`, `reaper` or
`renoise`) closes Kumi and opens it again with that one. `--daw` and `KUMI_DAW`
always decide, and Kumi then stays with that DAW.

**Where Kumi looks.**

- **REAPER:** its resource folder (`~/Library/Application Support/REAPER` on a
  Mac, `%APPDATA%\REAPER` on Windows, `~/.config/REAPER` or `~/.reaper` on
  Linux), each portable REAPER in the usual app folders (a `reaper.ini` beside
  the app), any REAPER that's open, wherever it is, the one Kumi's extension
  went in, or only the folder `KUMI_REAPER_RESOURCE_DIR` names. A portable
  REAPER elsewhere (on another drive, say) is found while it's open; the
  extension goes in the REAPER that's open.
- **Renoise:** the newest preferences folder for 3.5 or later (such as
  `V3.5.4` in `~/Library/Preferences/Renoise`, `%APPDATA%\Renoise`, or
  `~/.config/Renoise` or `~/.renoise` on Linux), or the one
  `KUMI_RENOISE_PREFS_DIR` names: a `V<x.y.z>` folder, or the folder that holds
  them.

**Connected.** Once the DAW runs with the bridge, Kumi connects by itself, and
again whenever the DAW comes back. While the DAW is busy (a dialog, a render),
Kumi says so and changes nothing until it answers.

**What works there.** Kumi reads the project or song, changes it with undo
([REAPER and Renoise](KUMI_CHANGES.md#reaper-and-renoise) lists what it can
change and what undo does there), and FOCUS, NOW and HISTORY follow the DAW's
own views ([commands, keys and screens](KUMI_TUI.md#reaper-and-renoise)). It
listens, judges its changes, works with `/loop` and `/goal`, matches
references, takes instant phrases, catches up on what changed while it was
closed, turns what you do by hand into recipes, and uses your library, notes
and conversations, all in the DAW's words ([what Kumi does in Live, REAPER and
Renoise](DAWS.md) compares them). In REAPER it can also run REAPER's
actions, its API and Lua (ReaScript); in Renoise, Lua. These stay Live's: Max
for Live devices, Kumi Ears, Willington, Kumi's Live extension (with **Ask
Kumi about this**), Live's own commands and Live's manual.

**Versions.** Each Kumi release ships with an extension and a tool, which
`kumi update` brings up to date ([updating](#updating-reporting-and-uninstalling)).
Kumi 1.24.10 to 1.24.28 with REAPER extension 0.1.7, 1.24.0 to 1.24.9 with 0.1.6, 1.23.3 with 0.1.5, 1.23.2 with 0.1.4, 1.23.1 with 0.1.3, 1.21.2 to 1.23.0 with 0.1.2, 1.21.0 to 1.21.1 with 0.1.1, 1.20.0 with 0.1.0.
Kumi 1.24.10 to 1.24.28 with Renoise tool 1.0.6, 1.24.0 to 1.24.9 with 1.0.5, 1.23.3 with 1.0.4, 1.23.2 with 1.0.3, 1.23.1 with 1.0.2, 1.21.0 to 1.23.0 with 1.0.1, 1.20.0 with 1.0.0.

## Working with Kumi

Kumi runs full screen: the conversation on the left, a Live pane on the right
and the input box at the bottom. FOCUS follows what you touch in Live, NOW shows
what Kumi is doing, and HISTORY lists every change with its undo. Below 100
columns the pane is a drawer under the conversation, which opens to each change
Kumi makes, and Ctrl-\\ opens and closes it. `KUMI_UI=plain`, or piping the
output, gives plain line-by-line output instead, which suits screen readers.
[Commands, keys and screens](KUMI_TUI.md) has the details.

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

## Background jobs

Kumi does slow work that doesn't change the Set in the background, so you can
keep working with it: research on the web, a tutorial video, Live's manual, your
library or earlier conversations, or a Max for Live device's file.

- **Starting:** Kumi says what's running in a line and is free for your next
  message. Ask for one ("look into this in the background") or let Kumi decide.
- **Seeing:** the header counts the jobs at work (◐ 2 jobs), and the **JOBS** tab
  lists each with what it's doing, its time and its tokens. `/jobs` lists them
  too.
- **When one ends:** a card in the conversation gives its first line ("✓ Job 2 ·
  Burial vocal chops · 5 steps, 3 sources"), and Kumi reads its report with your
  next message. A job your request needs ("watch this and make the bass like
  theirs") carries on by itself once you're not picking, typing or in a panel.
- **Live stays Kumi's:** a job researches and writes but never reads or changes
  Live. Kumi makes any change itself, in HISTORY with its undo. What a job reads
  is data, not instructions.
- **Plug-in presets:** jobs can use the built-in VST3 tool to create and edit
  supported preset files, with the same version and write checks as the main
  conversation. They return file paths; Kumi loads, reads back and auditions the
  presets in the foreground. This works with Live, REAPER and Renoise.
- **Ctrl-B** moves the answer's slow step that doesn't use Live (a video, pages)
  to the background, and the answer carries on. In tmux, press Ctrl-B twice.
- **Stopping:** `x` on a JOBS row or in a job's details. Esc stops the answer,
  not the jobs. Quitting interrupts them; `/jobs` runs them again next time.
- **Away:** while Kumi's window is behind another, a job's end is a notification
  and a line in Live's status bar, and the computer stays awake while jobs run.
- **Settings:** `maxJobs` (three at once) and `workerModel` (a model from
  `/model`'s list for the jobs; the conversation's by default). Each job's
  tokens show in its row.

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
  When Live opens a dialog (Export, say), Kumi reads it, check boxes included,
  and answers it.
- To separate stems, Kumi ticks the stems you ask for in Live's dialog (vocals,
  drums, bass, others; all four unless you say), puts two or three on one track
  if you ask, and picks High Quality or High Speed when you say (otherwise the
  one you used last). It presses Separate and waits while Live works: a minute
  or more for a whole song.
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
  leaves the transport alone. A whole-song listen plays the song from the start
  and leaves Live stopped.
- **While Live is stopped**, it plays the loop (or a few bars from the playhead,
  the part you name, or the whole song to its last clip). A listen takes as long
  as the music it hears.
- **You hear what Kumi hears.** While it listens, Kumi never turns Main down or
  mutes a track it's working on, so you can follow along and steer. Each listen
  says which bars it plays and why ("after the cut at 300 Hz").
- **Several tracks at once:** each one's sound, and where two sit in the same
  band at similar levels.
- **How:** Kumi Ears, a small Max for Live device Kumi brings (`kumi bridge` puts
  it in your User Library's Kumi folder). Kumi places it at the end of a track's
  chain when it needs to hear it and takes it away after. Sound passes through
  it unchanged, except a candidate it holds back (below), and nothing is
  recorded into your Set.
- Auditions and the sound-match search hear their candidates the same way: no
  scratch tracks, no arming. In an audition you hear the first candidate, and
  Kumi Ears holds the others back while Kumi records them. The search holds all
  its candidates back and puts its best on "Kumi · Goal best" when it pauses or
  ends. Without Max for Live, Kumi records to listen instead, and the candidates
  play together.

**Hearing a REAPER project or a Renoise song.** Kumi renders the part offline
(faster than it plays, with a bar before it), hears that, then plays it out
loud for you (up to a minute). A track is rendered on its own: REAPER's
pre-fader stem, or Renoise's track solo.

- **While REAPER plays**, Kumi can hear the master output as it plays (8
  seconds by default); a track on its own is rendered.
- **While Renoise plays**, a render would stop its playback, and Renoise's
  scripts can't hear what it plays: Kumi renders only a part you name.
- **Judged changes** keep their level with the last ReaLimit, else a JS: Volume
  Adjustment Kumi adds (REAPER), or the last Maximizer, else a Gainer (Renoise).
  Goals and the sound-match search try their candidates on scratch copies of the
  track ("Kumi · try …"), which Kumi takes away after; the best stays on
  "Kumi · Goal best".
- Renoise's demo can't render, so Kumi can't listen there; `kumi doctor` says
  so.

## Judging changes and the loop

For work that can be measured (mastering to a loudness, taming a harsh
resonance, making the vocal cut through), Kumi judges its own changes instead of
trusting them:

- **A goal becomes a checklist:** a loudness and a ceiling, a balance, a
  reference's profile, or the element that must cut through, each with a
  tolerance. Kumi first hears the whole stretch and lists what's off: harsh or
  resonant frequencies, wide lows, rumble, DC, a loud bass note, overs and
  clipping, and what masks the element you name.
- **Masking is read before Main's chain.** Kumi hears the element before
  Main's chain. With Max for Live it also hears the mix as it comes into Main's
  chain (a second Kumi Ears device, first on Main, taken off again after each
  listen), so masking reads fairly whatever Main holds: a limiter or an EQ there
  doesn't move it. Without Max for Live it hears only the mix after Main's
  chain, so masking is on the checklist only while Main's devices are off,
  meters (Spectrum, Tuner) or a Utility that only turns the level (the element
  is heard at that level too). With anything else on Main it's left off and the
  run says so: judge the element cutting through with Main's devices switched
  off, then the master's loudness and peaks on their own.
- **One change at a time:** after each change Kumi brings the level back where it
  belongs (the goal's loudness, or where it was) with the last Live Limiter on
  the track, else a Utility it adds, so before and after are judged at the same
  loudness. It keeps the change only if its target improved and nothing else got
  audibly worse (a measure it can no longer read, such as silence, counts as
  worse); if the peaks come back or it clips at that level, the change goes too.
  A change that isn't kept, Kumi's own undo takes back.
- **Code picks the numbers.** Kumi chooses the change; code finds its values,
  with as few listens as it can: an EQ calculated from what was measured (the
  smallest cut that clears a resonance, or bands toward a reference's shape),
  one knob homed in on (a limiter's gain until the loudness is right, a
  de-esser's threshold), one knob probed across its range (seven settings
  heard side by side on scratch copies of the track in one pass, then set where
  its response meets the target), or a few knobs that interact searched in
  small generations, each heard the same way. Probing and searching need a run
  judged on one track (`judge` with `track`), not the mix. What a knob was heard
  to do on a track is remembered for a month, so the next time it's homed in on
  there, Kumi starts where that says.
  Every added device and every move has to earn its place.
- **The round log** shows each round in the conversation: what it was after, the
  change, the numbers before → after, kept or taken back and why, and what's
  next, with the listens so far. A target that resists two changes in a row
  waits while Kumi works on the next one.
- **A listening model**, when there is one, hears before and after too, as a
  mono mix (so it isn't asked about width or air). It can turn down a change it
  hears as distorted, pumping or clipped when it also prefers the take before,
  both ways round; the meters decide everything else. With a Gemini API key
  (`GEMINI_API_KEY`) it's Gemini's newest Flash model, picked from Gemini's own
  list; else, with an OpenAI API key, OpenAI's newest audio model.
  `KUMI_LISTENER` names another (`<base URL>#<model>`, such as a model server on
  your computer), and `KUMI_LISTENER=off` (or any value that isn't a model's
  address) leaves the meters alone. A run's first round says which model will
  listen.
- **Model slots.** `/slots` shows which model does each listening job: stems
  (Live's own splitter), transcription (Live's conversions), listening (the
  lookup above) and embeddings (Kumi's own: LAION-CLAP's music model for style,
  AFx-Rep for effects). Swap one in plain words, such as
  `/slots listening gemini`, `/slots listening off` (at once, mid-run too) or
  `/slots listening http://127.0.0.1:8080/v1#<model>`. A new listening model is
  tried on a known clip first, and switched to only if it hears it right.
  `/slots back <job>` takes a swap back. Embeddings run in Kumi's own model
  runtime: `/slots embeddings ~/models/clap.onnx` (quote a path with spaces) or
  an https link to an `.onnx` file on any host (up to 4 GB; Esc stops the
  download) is tried on two known tones and used only if it tells them apart. It
  takes the style model's place; the effects model stays Kumi's own. A file
  that's gone later gives way to Kumi's own, said once; `/slots embeddings off`
  stops them. Stems and transcription can't take model files yet. A
  `slots.json` Kumi can't read leaves listening off. `KUMI_LISTENER` wins over
  the listening slot while it's set, but `/slots listening off` still stops it.
- **Style and effects, heard by models.** With a reference, Kumi's style model
  (LAION-CLAP) hears it and each listen of the run, and a change that moves the
  sound further from the reference's style is taken back. For a sound goal, its
  effects model (AFx-Rep) does the same for the sound's effects. They're fetched
  once, the first time they're needed (ONNX Runtime about 10 MB, 80 MB on
  Windows; the models about 245 MB, each checked against its checksum), into
  `models` in Kumi's folder (`~/.kumi/models`, or under `KUMI_HOME`), when the
  disk has room for them.

**References** become targets too. Give Kumi a file, a folder of them, a YouTube
video or playlist, a Spotify link, or just words ("like Burial", "dub techno",
"Kid A"): it finds example tracks (an artist's best-known songs, an album's
tracks, a genre's main artists), measures each (passing over silent or quiet
tracks, under −45 LUFS, and, in a folder or a search, sketches under 30
seconds), and keeps a profile with the range the tracks keep to, so "in the
style" means inside it. When the words could mean more than one thing (two
artists by one name, or an album and an artist), Kumi asks you once which you
mean, and your answer settles it. Each reference is measured once and kept in
`~/.kumi/references`; a file or folder is measured again once its files change.
For one element of a reference (its bass, its drums), Kumi can put the track in
your Set, separate its stems with Live's own splitter (Kumi may ask you to click
the track's clip), and measure a stem.

**A part's feel** is judged on its notes, with no listening: where it plays in
the bar (in five lanes for a part on a Drum Rack: kick, snare, hats, cymbals and
percussion), how early or late each step sits (its push and swing), its accents,
syncopation and fills, against a reference's MIDI. For a record's drums, Kumi
separates its stems (you click the track's clip when Kumi asks), converts the
drum stem to MIDI with Live's Drums to MIDI, and moves each hit onto the stem's
real onset first. A pitched part that isn't in Live (a bass line in an audio
file, up to two minutes of it) becomes notes with Basic Pitch, on a grid fitted
to them (its tempo and first downbeat); from audio, its timing and swing are
compared, not its accents. Fills need 8 bars or more. Kumi can move your notes
toward the reference itself, step by step, and keeps a change only if it got
closer without anything else moving away.

**A song's form** is heard bar by bar: the energy curve (loudness, density,
brightness, low end), the sections with their roles and repeats, the turns
between them and what prepares each drop, the intro, outro and first hook, and
which tracks play where (up to 64 tracks). Kumi also points out what doesn't
work: sections without contrast, an energy plateau, odd phrase lengths, a drop
that arrives unprepared, a loop left unchanged for 32 bars or more. Against a
reference song, it says how the two forms differ.

**A sound** is measured beside the others in one pass: its attack, its
decay against the beat, brightness, an 808's or a kick's pitch drop, a wobble's
rate in beats, warmth (2nd and 3rd harmonics), width, top, noise floor, how long
its tail hangs on, and crackle. Kumi points out clicks at note edges, DC and
notes that jump out. A kit is judged as one: a piece whose noise floor, top or
grit sits apart, pieces that pile up or leave a gap, decays ringing into the
next hit. Against a kick, it says how far each part ducks and how fast it comes
back, and whether their hits interlock or collide; with the key, how far a bass
line's notes sit from it. Effects are read off what's heard too: a reverb's
decay time and how its tails darken and widen, echoes as a note value with their
repeats and how far each falls, how deep a level swing goes, how far a filter
sweeps, pumping against the beat, and how much of the sound is tail, bar by bar.
Kumi flags tails cut off, echoes out of time, repeats that don't die away and
tails burying the dry hits. Ask for a sound or an effect like a reference and
Kumi closes those measured gaps round by round, keeping only what got closer.

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
(sign-in, billing, the model, a setting, Live, a plan's limit more than 6 hours
away) stops it as blocked, naming what to do; a nearer limit is waited out, or
your fallback carries on. When a turn ends on a question with an option Kumi
recommends and nobody answers, Kumi takes it after a minute (`answerAfter`) and
the goal carries on (`/goal answer: <words>` does the same by hand). A message of yours while it runs is answered, then the goal waits for
you. The GOAL tab shows the goal, its turns and time against the budget, the
last check and what's next. `/goal` alone shows it; `/goal edit <words>` changes
it, `/goal resume` carries on (with a fresh budget for whatever ran out),
`/goal pause` or Esc pauses it, and `/goal clear` ends it (with no unfinished
goal, it ends a paused sound-match search). Another `/goal` while one is
unfinished asks first: send it again,
or `/goal new: <words>`, to replace it. A goal is kept with its Set, so it's
still there after a restart. An unsaved Set's goal is kept by Live's identity
for it while the Set stays open (through a restart of Kumi or a new
conversation), and moves with the Set's first save.

## Matching a reference

Ask Kumi to make something sound like a reference ("make the bass sound like
this: ~/refs/bass.wav") and it treats it as a search, not a guess. It listens to
the reference, builds two to four different takes on their own tracks, and
renders them together to score each against the reference (0 to 100, with the
biggest differences); you hear the first take, and Kumi hears the rest on its
own. It then refines the best, changes structure when
no knob closes a gap, and stops when it reaches the target, when new ideas stop
helping, or after 12 rounds or 45 minutes. It ends with the score before and
after and what still differs.

`/loop` and a sound to match goes further: Kumi keeps searching, mostly with
its own fast knob search and with the model's bigger ideas every few
generations, until the score reaches 95, you stop it, or four hours pass. The
GOAL tab shows how it's going. With Max for Live, Kumi holds the candidates back
while it tries them, so you hear the Set without them; the best goes on
"Kumi · Goal best" when the search pauses or ends. Esc pauses the search,
`/loop` alone picks it up again (even after a restart), and `/loop stop` ends
it.

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

- **Built-in VST3 presets.** Ask Kumi to create or edit a preset. It checks its local format catalog before external research, and can load supported presets with history and undo. Serum 2, Ozone 12 and its modules, Valhalla, Minimal Audio, Pulsar Modular and other catalogued products have versioned format support; see [capabilities and limits](../../plugin-formats/README.md).
- **Installed processors first.** Mastering and reference mastering check Ozone 12, then other suitable installed VST3s, then native devices. Kumi measures and listens before and after; a reference is compared at matched loudness.
- **Native file limits.** Softube and some Zynaptiq presets support inspection, template copies and renaming; unverified numeric controls remain read-only. VST3 state loading is not claimed for locally unverified products. Live loads a new preset instance from its browser; existing instances use exposed parameters.
- **Verified writes.** The installed version must match a tested writer. Numeric edits need a verified write range; text and Boolean edits need verified semantics or an explicit allowlist. Resource paths stay read-only. New files stay in Kumi's preset folder; Live also saves a User Library copy, which remains when undo removes the loaded device.

Kumi also has detailed guides for Serum 2, Vital, Ozone 12, Pro-Q 4, Pro-L 2,
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
  limiter's gain, its ceiling, an EQ band's gain, a compressor's threshold). On
  EQ Eight, an EQ band is the nearest band that's on (a bell or a shelf for a
  gain), within about an octave of the frequency the change is after; otherwise
  Kumi asks which. It finds the knob among the names the plug-in shows Live and
  turns it only when it's configured. When it isn't, Kumi says which knob to
  configure, and has another device on the same track do the job meanwhile:
  Ozone 12 first, then the other plug-ins Kumi knows, then Live's own devices.
  With none on the track, Kumi checks suitable installed VST3s before Live's own devices: a
  limiter last, an EQ or a compressor before the last limiter.
- **Beyond exposed parameters.** Kumi checks the built-in preset format for supported settings first.
  Controls outside that format and interactive features such as Ozone's Master Assistant still use the
  plug-in's window; the guide says where.
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
original works. The device lands in your User Library's Kumi folder, under
Audio Effects, MIDI Effects or Instruments, where Live's Browser lists it like
any other, with as many knobs as it needs (in up to three rows). They're
ordinary Live parameters, so you can automate and map them. Loading it is a
change in HISTORY with its undo.

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

## Your music

Kumi can learn your taste from your own music, on your computer, and lean on it
when you leave the style open: "make me a beat", "something dark". When you name
a reference, sounds or values, it goes by those instead.

- **Asked once.** The first time a request leaves the style open, Kumi may say
  how many songs it found (in Music's folder, iTunes' or `~/Music`; on Windows,
  your Music folder) and ask whether to learn your taste from them. Say no and it
  doesn't ask again; `/music add <folder>` starts it any time.
- **What it learns.** Each song's tags (artist, album, title, genre, year), or
  names from its folders when it has none; then, in the background, how it
  sounds: its loudness, brightness, low end, dynamics and room, and a style
  fingerprint. A spread of songs across your artists is heard first (about an
  hour), then every song for a minute, newest first, then each in full.
- **Out of the way.** It runs in Kumi's library learner at the lowest priority,
  waits while your DAW plays (Kumi's listening included) and while your computer
  runs on its battery, and carries on where it stopped. Songs under 30 seconds
  or near silence are skipped; Apple's protected purchases are named but never
  heard.
- **Like your own music.** Ask for something in your own style ("like my
  stuff") and Kumi keeps your nearest songs (5 to 10, heard in full) as a
  reference, then works toward it as it does toward any reference.
- **In your words.** Describe a style ("melancholic dub techno") and Kumi finds
  the songs in your music that sound like it, with the style model's words side
  (fetched the first time, about 127 MB). It names your styles the same way,
  from about 200 genre and mood words. With another embeddings model in
  `/slots`, words go by your songs' genre tags instead, and Kumi says so.
- **What the model sees.** The names of the songs nearest what you asked for
  (artist, title, album) and a sentence about what they share, never your files
  or their fingerprints. Song and artist names reach the model when Kumi uses
  your taste.
- **Where it's kept.** `~/.kumi/music`, readable only by you. `/music` shows the
  folders, how many songs are heard and how it's going; it adds or removes a
  folder, pauses and resumes, and `/music forget` deletes all of it.
- **On a map.** `/map` draws your songs by style, and your library's sounds by
  class, songs that sound alike side by side. Move around with the arrows, and
  hear a point with Space or by moving the mouse onto it
  ([the keys](KUMI_TUI.md)). Pick sounds there (drag a box, or `x`) and Enter
  puts them into your palette, kept between sessions (`/palette`). Kumi reads
  your palette with each request, so you can ask for "a darker kick from my
  palette".
- **In your browser.** `/map open` shows the same map in your browser: a name
  when you point at a point, the wheel to zoom, Shift and drag (or Lasso) to pick
  points, a filter, and a click to hear one. Its actions put their message in
  Kumi's input box, and Add to palette puts the points picked into your palette. The page comes from your computer only
  (127.0.0.1, at a link with a random key) and plays a point by its place on the
  map, never by a file's path. `/map close` closes it, and so does quitting
  Kumi; over SSH, Kumi says how to reach it.

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
  40, in `~/.kumi/techniques.json`. Techniques aren't kept per DAW: one learned
  in Live is offered in REAPER or Renoise as an idea to adapt.
- **Recipes** (↻): ways of working you can replay in any Set, such as a vocal
  chain or a resampling loop. Ask Kumi to keep what it just did, describe a
  routine, or say "watch me", do it by hand in Live and say when you're done:
  Kumi turns what changed into a recipe with blanks for what differs each time.
  Ask for a recipe by name to run it, or use `/recipes`. Kept in
  `~/.kumi/recipes`, one file each. REAPER's and Renoise's are kept apart
  (`~/.kumi/recipes/reaper`, `~/.kumi/recipes/renoise`), made of that DAW's
  changes, and offered and run only there.
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

Kumi keeps each Set's conversations in `~/.kumi/projects`, saved after every
answer, each up to about 256 KB (the oldest exchanges drop off). Beside each one,
Kumi also keeps a log of what happened in it, up to 32 MB, with keys taken out of
its words. Opening Kumi on a saved Set carries on its latest conversation, with
its last 100 changes in HISTORY (without undo). `/new` starts afresh and keeps the last one. An unsaved
Set's conversation moves to the Set's own folder when you first save it.

- **Going back:** `/resume` (or `/conversations`) lists this Set's
  conversations, and Tab shows every Set's. Type to filter; Enter goes back to
  one; ctrl+f renames, pins or deletes it, or copies its id.
- **From the shell:** `kumi -c` carries on the conversation you had last, in
  any Set. `kumi -r <id or name>` carries on that one (the start of an id is
  enough), and `kumi -r` alone lets you choose. Both work with
  `--inference-only` too.
- **Ids:** every conversation has one, such as `mv66me0m3fa91c`. `/status`
  shows it, and as Kumi closes it says "Continue this conversation:
  kumi -r mv66me0m3fa91c" after a run that used it.
- **Names and copies:** `/rename <name>` names this conversation, `/new <name>`
  names a new one, and `/fork` carries on in a copy (the original stays).
- **Another Set's conversation:** Kumi asks first: open its Set in Live (Live
  may ask about the open Set), carry it on here as a copy (without what Kumi
  read of the other Set), or cancel. With Live closed, it carries on, and Kumi
  asks once Live opens another Set.
- **Two Kumi windows:** a conversation open in one carries on in a copy in the
  other.
- **The model:** going back keeps the model you use now; Kumi says once when
  the conversation was had with another.
- **Keeping:** each Set keeps its 20 newest conversations, any used in the last
  90 days (`conversations.keepDays`; `0` keeps all) and the pinned ones. With
  `keepDays` taken out of settings, Kumi keeps the longer of 90 days and the
  value last set (`0` still keeps all). After a rollback, a Kumi 1.16 or
  earlier keeps only each Set's 20 newest, and so does a Kumi window opened
  before the update, until it closes.
- **Sharing:** `/export` copies this conversation as Markdown, or saves it as
  Markdown, a web page or a file for Kumi (`/export copy`, `md`, `html` or
  `json` does one straight away). `/share` saves the web page, then opens it,
  shows it in Finder (Explorer) or copies its path. Files go in the folder you
  started Kumi in, named after the conversation, never over another file; on
  macOS and Linux, only you can read them.
- **What an export holds:** what you and Kumi said, each step by name, and the
  changes from HISTORY (with pictures on the web page). Keys, tokens, your home
  folder and your account name are taken out. Live's details, your notes,
  Kumi's notes to the model and the model's reasoning never leave. The web page
  loads nothing from the internet.
- **From the shell:** `kumi export [id or name]` saves Markdown (`--md`, the
  default; `--html`, `--json`); without an id (or with `--last`), your latest
  conversation. `--full` adds each step's input and result, `-o <path>` chooses
  where, and `--raw` leaves keys and paths in (and says so).
  `kumi import <file>` keeps a file saved with `--json`; `kumi -r <id>` carries
  it on in a copy, and Kumi tells the model it came from a file.

When you mention something from before ("the reverb chain from last week's
vocal"), Kumi looks through the conversations kept for every Set, and its
techniques and recipes, by their words. The search runs on your computer; what
it finds goes to the model like any other read.

Kumi also remembers each saved Set as it last saw it. Next time, the welcome
screen says what changed meanwhile ("Since you were last here · 3 days ago:
Tempo 120 → 124 BPM; Added track “Pad”"), and Kumi takes it into account. Kumi
knows a Set by an id it keeps inside it, so a Set moved, or saved under another
name in the same Live Project folder, carries on; a copy saved elsewhere starts
afresh.

**In REAPER and Renoise** the same holds for each saved project or song:

- Its conversations, notes and catch-up are its own. An unsaved one keeps them
  in the DAW's unsaved place until its first save; then they go on in the saved
  one's.
- Kumi writes an id into a saved REAPER project (REAPER keeps it with your next
  save), so a project moved, or saved under another name in the same folder,
  carries on; a copy saved in another folder while the original is still there
  starts afresh.
- Another project's conversation: Kumi asks you to open that project yourself
  (it can't open one there), and the conversation carries on once it's open.
- Catch-up also tells what changed while REAPER or Renoise was closed or
  restarted, in the DAW's words: REAPER's tracks, FX, items, markers and
  regions, tempo and time signature; Renoise's tracks, devices, patterns,
  instruments, tempo, lines per beat and pattern sequence.
- The state it compares with is in `~/.kumi/bridges/reaper/snapshots` (or
  `renoise`), readable only by you. Deleting it only makes the next catch-up
  start afresh.

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

**REAPER and Renoise** going away is said the same way: Kumi chats without the
DAW and connects again by itself once it answers ("REAPER answered, so Kumi is
connected to it now."). Kumi's undo there keeps its newest 400 changes, as long
as Kumi's link to the DAW lasts: after a `/reconnect` or a Kumi restart, earlier
changes read **no undo**, and each says whether the DAW's own undo may still
take it back.

## Updating, reporting and uninstalling

```sh
kumi update                    # the newest Kumi and its bridge
kumi update --check            # only say whether there's a newer Kumi
kumi update --rollback         # go back to the Kumi before the last update
kumi update --version 1.15.0   # that release, newer or older; --rollback returns
kumi doctor                    # check the installation, sign-in, the bridge, Live and the terminal
kumi doctor --json             # the same checks as JSON, without keys or your home folder
kumi report                    # a file to send when something goes wrong
kumi uninstall --dry-run       # say what uninstall would remove
kumi uninstall                 # remove Kumi; add --all to remove what you kept in it too
```

`update` fetches the newest release, checks it against its checksum and starts
it once to be sure it runs before putting it in place; the one before is kept
for `--rollback`.

- It needs about 250 MB free, and says so before downloading.
- When it can't finish, it says why (the connection's cause, a Wi-Fi sign-in
  page answering instead of GitHub, a full disk) and that nothing was changed.
- If `~/.kumi` belongs to another account (an install run with `sudo`), it
  gives the `sudo chown` line that takes it back.
- It ends with the versions (`1.15.0 → 1.16.0`) and a few lines on what's new;
  `/changelog` in Kumi has the rest. Kumi windows already open are mentioned
  only when one is.
- `--version x.y.z` installs that release, newer or older (1.7.6 or later). The
  Kumi you had is kept, so `--rollback` returns to it. Going back leaves the
  newer bridge in Live, which works with the earlier Kumi.

If the bridge needs an update or a switch from JavaScript to native and Live is closed, it then runs
`kumi bridge --yes`; if Live is open, it says to run `kumi`, which can restart Live for you. In a
copy of the repository, `update` moves the checkout forward instead
(`git merge --ff-only`, refusing local changes) and builds the workspace with Cargo.
Inside Kumi, `/update` asks first, then closes Kumi, updates it and opens it
again with the same conversation.

`--rollback` goes back to the Kumi before, and puts back its bridge too when Live is closed; in a
terminal, it offers to wait while you quit Live (Kumi never quits Live itself). Otherwise the newer
bridge stays, which works with the earlier Kumi: to put back its bridge as well, quit Live and run
`kumi update --rollback` twice.

**REAPER's extension and Renoise's tool** follow Kumi, wherever Kumi's bridge is in:

- `kumi update` puts in the copy the new Kumi carries and says to restart the DAW if it's open
  ("Kumi's extension 0.1.1 is in REAPER now: restart REAPER if it's open."). Each goes where Kumi
  put it, and a copy a newer Renoise made of the tool in its own version folder is brought along. One
  you took out isn't put back ("Kumi's tool isn't in Renoise any more"), and an extension whose
  REAPER folder is gone never moves into another REAPER: `kumi --daw reaper bridge` puts it in.
- `--rollback` puts back the copies the earlier Kumi carried. A Kumi that doesn't know REAPER or
  Renoise (1.19.3 and earlier) leaves them where they are, unused, and Kumi names the file or folder
  to delete to take them out. That Kumi won't start while `KUMI_DAW` is `reaper` or `renoise`: unset
  it to use it.
- A copy that changed since Kumi put it in the DAW's folder is first kept in
  `~/.kumi/bridges/reaper/changed` (or `renoise`), and Kumi says where.
- Neither needs Live closed, nor the DAW: a running DAW loads the new copy when it restarts.

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

`doctor` starts with the installation:

- Kumi's version and folder, and what `--rollback` goes back to.
- Whether the `kumi` your shell finds is this one.
- Whether a newer Kumi is out, from the daily check (it doesn't ask the network).
- Free space, the Lives installed, Live's User Library, the bridge's ports
  (9765 and 9766) and Live's Developer Mode.

Each problem names one next step. `doctor` exits with 1 when something needs
fixing; notes (no Live on this computer, a newer Kumi) don't count.

Where REAPER or Renoise has been used, `doctor` checks it too, in its words
(`kumi --daw reaper doctor` speaks of that one):

- the DAW, its version and where it is, and whether it has been opened;
- Kumi's bridge in it: its version and checksum, whether it changed or went
  since Kumi put it there, an older one than this Kumi's, its reference file and
  secret, macOS's download mark on it, and the extension's architecture;
- whether the DAW runs and its bridge answers (with the project or song open),
  a DAW still running an older bridge (restart it), and another program on its
  ports (REAPER 9770–9779, Renoise 9780–9789);
- for Renoise: its demo (it can't render, so Kumi can't listen offline), its OSC
  server (when on, it takes commands from the network without a password), and
  Kumi's tool turned off in Tools › Tool Browser.

`--json` prints the same checks for sharing, with keys, tokens, your home folder
and account name taken out, as in a report. Its wording can change between
versions, so it isn't meant for scripts to match:

- `kumi`: the version; `fixes`: how many checks need fixing.
- `sections`: each a `title` and its `checks`, each with a `status` (`ok`,
  `note` or `fix`), its `text` and, when there is one, the `next` step.
- The exit code is the same as without `--json`.

`report` writes `~/kumi-report-<date and time>.txt`: Kumi's and the bridge's
versions, the doctor's checks, your settings, what Kumi did in your last
conversation, the gap log, and the bridge's lines from Live's own log (for
REAPER and Renoise, the last lines of Kumi's bridge logs in
`~/.kumi/bridges/<daw>/logs`). Keys and tokens are taken out, your home folder
shows as `~` and your account name as `<user>`. Read it before sending.

`uninstall` removes Kumi, the one kept for `--rollback`, what updates leave
behind, any retained legacy Node runtime, its launcher, the PATH entry it added
and what Kumi fetched (programs, models and watched videos, fetched again when
needed), and says how much space that frees. It offers to take the bridge and
the extension out of Live (only while Live is closed), Kumi's extension out of
REAPER and its tool out of Renoise. Your conversations,
notes, recipes, sign-ins, library and references stay unless you add `--all`,
and so does catch-up's state for REAPER and Renoise; copies of their bridges you
had changed (in `~/.kumi/bridges/<daw>/changed`) always stay.
`--dry-run` says what it would remove, and removes nothing.

- It removes nothing when Kumi's folder (`KUMI_HOME`) is your home folder, a
  disk's root, a relative path or a folder without Kumi's launcher and app, and
  says what of Kumi's to take out by hand. The installers won't install there.
  Run with `sudo`, it also leaves your home folder alone, and any folder that
  isn't root's.
- From `bin`, `downloads` and the folders of what Kumi fetched, only Kumi's own
  files go. Anything else stays and is named, and so does the folder.
- `--all` takes your conversations and the rest only from a folder named
  `.kumi`, as the installer names it, and the folder once it's empty: anything
  you put there yourself stays. From any other (`KUMI_HOME=~/.local`, say), only
  Kumi itself goes, and they stay there, named, for you to delete.
- When a part can't be removed, it says which and why, and Kumi stays so you
  can run `kumi uninstall` again.
- A PATH line it can't take out (in a file it can't write, inside an `if`, `for`
  or function, or joined to the code beside it by `&&`, `||`, `|` or `\`) is
  named with its lines, to take out by hand.

## Limits

| What | Limit |
| --- | --- |
| A message you send | 256 KB |
| One answer | 200 model steps; stops after 10 minutes without progress, or 60 minutes in all (longer for matching and goals; waiting for a plan's limit doesn't count) |
| Retries | Up to 3 for a busy or failing provider, before any output of that step was shown, waiting as long as it asks; a plan's limit up to 6 hours; no connection, for as long as the answer may run |
| A request to the bridge | 65 seconds |
| Changes in one answer | 5,000 (a batch of pads or parameters counts once) |
| A `wait` step in a plan | 30 minutes |
| Background jobs | 3 at once (`maxJobs`, up to 8); each 40 model steps and 20 minutes; a step moved with Ctrl-B keeps the answer's 60 minutes |
| Conversation size | Earlier Live reads shrink past about 160 KB; the oldest exchanges drop off past about 400 KB |
| Audio heard | The first 12 minutes of a file, up to an hour when asked; a whole song up to an hour |
| Video transcription | 90 minutes at a time |
| Free disk space checked first | 100 MB to record (on the Set's disk, or your home folder's for an unsaved Set), 100 MB to make a device (on the User Library's disk), twice a model's size and 200 MB more to fetch one of Kumi's own, a slot link's size and 200 MB more (in Kumi's folder) |

**Listening** hears files, recordings and the Set's tracks and mix (the Set
through Kumi Ears, which needs Max for Live; a REAPER project or Renoise song by
rendering it). It measures, compares and judges changes against a goal; taste
stays yours.

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
  from the Set, the frames of videos it watches, the pictures it reads, the
  pictures you add to a message, and what background jobs are given and read
  (`workerModel`'s provider, when it's set). With a
  model on your computer, they stay on it (a server named in `settings.json` gets
  them wherever it runs).
- **Web search and reading** go to the search services above, and the pages Kumi
  reads see its requests. Kumi doesn't read an address that carries a key or a
  token.
- **Downloads** come from GitHub (Kumi's releases and update checks, yt-dlp,
  ffmpeg and whisper.cpp, ONNX Runtime and Kumi's own models), Hugging Face (the
  speech model), any https address you give a slot's model, and the video sites
  you name.
- **References** in words are looked up by name on MusicBrainz and ListenBrainz,
  a Spotify link on Spotify's public page (an artist's on MusicBrainz), and
  their audio comes from a YouTube search and is deleted once it's measured.
- **Audio** is analysed on your computer; only the numbers go to the model. A
  listening model, when there is one, gets short excerpts (at most 10 seconds
  each, mono) of the changes it weighs: Gemini with a Gemini API key, OpenAI
  with an OpenAI API key, or the model in `KUMI_LISTENER` or the listening slot
  (`/slots`), with `KUMI_LISTENER_KEY` when it's set. `KUMI_LISTENER=off` or
  `/slots listening off` stops that; trying a new listening model sends it only
  a known test tone.
- **Your voice** is written down on your computer, and the recording deleted as
  soon as it is; only the words leave, when you send them.
- **Your library** is learned on your computer; only the manual's pages come from
  ableton.com.
- **Your music** is learned on your computer. When Kumi uses your taste, the
  names of the songs it picks (artist, title, album) and a sentence about how
  they sound go to your model provider, never the files.
- **models.dev's catalog** of models is fetched at most once a day; nothing of
  yours goes with the request. `"modelCatalog": false` stops it.

Track, clip and device names, tool results and web pages are data for Kumi,
never instructions. Words they hide in characters that don't show on screen
reach the model as `[hidden text removed]`. Kumi keeps its files in `~/.kumi`,
readable only by you; sign-ins are in `~/.kumi/auth.json`. Terminal scrollback
and your provider's own retention are separate.

## Proxies and school or office networks

- **A proxy:** Kumi goes through `HTTPS_PROXY`, `HTTP_PROXY` or `ALL_PROXY`
  (`http://…`, or `socks5://…`), except for `NO_PROXY`'s addresses. Without
  them, it uses the proxy set in macOS's or Windows' settings, with its
  exceptions. Kumi doesn't read a proxy auto-config (PAC) file: set
  `HTTPS_PROXY` to the proxy's address instead.
- **A proxy that wants a password:** `HTTPS_PROXY=http://user:pass@host:port`.
  When the system's proxy wants a sign-in Kumi can't give (NTLM, Kerberos),
  isn't answering, or shows a certificate your computer doesn't trust, Kumi
  tries directly instead. `NO_PROXY=*` always connects directly.
- **Never through a proxy:** models on your computer (Ollama, LM Studio) and
  Live's bridge. A model server you name (`modelServers`, `KUMI_LISTENER`)
  goes through the variables' proxy only, never the system's. With the
  system's settings, addresses on your own network go directly too.
- **A network that inspects secure connections,** as schools and offices often
  have: Kumi trusts the certificates your computer trusts, so it works once the
  network's certificate is installed, as your browser needs. `SSL_CERT_FILE`
  names a file of more certificates for Kumi to trust; as OpenSSL reads it, it
  stands in for the computer's own list on Linux and for web search and
  reading.
- **Web search and reading** connect directly, never through a proxy. Kumi
  checks that each address is public before it connects, and a proxy would look
  the name up again where Kumi can't check it. On a network that lets only a
  proxy out, they can't read pages.

## Files and settings

Kumi keeps everything in `~/.kumi`. `~/.kumi/settings.json` holds:

| Key | Meaning |
| --- | --- |
| `model` | The chosen model, `<provider>/<model>` (`/model`, `kumi model`) |
| `effort` | `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`; absent means the model's default (`/effort`) |
| `fast` | `true` asks for the model's faster tier where its provider lists one (`/fast`) |
| `fallbackModel` | The model that carries on when yours hits its plan's limit or its provider is down, `<provider>/<model>` (ctrl+f in `/model`) |
| `panelTab` | The Live pane's tab you had open last |
| `mapHover` | `false`: in `/map`, a point doesn't play as the mouse comes to it; `h` in the map turns that off and on, and Kumi keeps it here |
| `updateCheck` | `false` turns off the check for a newer version at start |
| `whatsNew` | `false` turns off What's new as Kumi first starts after an update |
| `notifications` | `"off"`: no desktop notification (or bell) while Kumi's window is behind another; `"auto"` otherwise |
| `liveStatusBar` | `false`: no line in Live's (or Renoise's) status bar when Kumi finishes or needs you |
| `keepAwake` | `false`: the computer may sleep while Kumi works |
| `answerAfter` | Seconds before Kumi takes its recommended answer when nobody's there (60); `0` never does |
| `theme` | `system` (the default: light or dark as the terminal's background is, turning with it where the terminal says its colours changed), `dark` (Kumi's own), `light` (made for a light terminal) or `colorblind` (the dark one in colours that stay apart for red-green colour blindness) |
| `instantPhrases` | `false`: fixed phrases ("tempo 124", "mute drums") go to the model like any other message ([instant phrases](KUMI_TUI.md#instant-phrases)) |
| `contextMeter` | The header's meter of how full the model's context is: `"auto"` (the default) from half full, `"always"`, or `"off"` |
| `modelServers` | Model servers you name: `[{ "name", "baseURL", "apiKey", "format" }]` ([models on your computer](#models-on-your-computer)) |
| `libraryFolders` | More folders for Kumi to learn sounds, presets and Sets from |
| `musicFolders` | The folders of your own music Kumi learns your taste from (`/music add`); absent until you agree |
| `musicOffered` | `true` once Kumi has asked whether to learn your music: it asks once |
| `voice` | Talking: `send` (send when you stop), `language`, `microphone` (`/voice`) |
| `maxJobs` | How many background jobs run at once: 1–8, three by default |
| `workerModel` | The model background jobs use, `<provider>/<model>` from `/model`'s list; the conversation's when absent |
| `conversations` | `keepDays`: days a conversation is kept after it was last used, beyond each Set's 20 newest (90; once taken out, the longer of 90 and the value last set); `0` keeps all |
| `modelCatalog` | `false`: Kumi doesn't fetch models.dev's catalog of models; the copy it has stays in use ([choosing a model](#sign-in-and-choose-a-model)) |
| `copyOnSelect` | `false`: text you drag across in Kumi's window stays selected until Ctrl-C or right-click copies it, instead of copying as you let go |
| `mouse` | `"off"`: Kumi leaves the mouse to the terminal (or tmux), whose own selection and copying then work; Page Up and Page Down scroll |
| `transcriptOnQuit` | `false`: quitting doesn't print the conversation into the terminal's scrollback |
| `fullRepaint` | `true`: Kumi writes every cell of its window each time something changes, for a terminal that draws only the changes wrongly. On by default in Windows Terminal, where `false` turns it off |

`~/.kumi/slots.json` keeps the model slots you swapped (`/slots`),
`~/.kumi/probes` what each knob was heard to do, by device and track, and
`~/.kumi/models` the model runtime and Kumi's own models, and
`~/.kumi/cache/models-catalog.json` the newest copy of models.dev's catalog. All
four follow `KUMI_HOME`.

`~/.kumi/bridges/reaper` and `~/.kumi/bridges/renoise` hold what Kumi keeps for
each DAW's bridge: its secret (`bridge.secret`), the files the running bridge
announces itself in (`discovery`), its log (`logs`), its install receipt
(`install-receipt.json`), the copies kept for rollback (`generations`), copies
you changed (`changed`), and catch-up's state (`snapshots`). `~/.kumi/last-daw` is the DAW you used last.

Environment variables (paths must be absolute):

| Variable | Meaning |
| --- | --- |
| `KUMI_MODEL` | `<provider>/<model>` for this run, overriding the chosen model |
| `KUMI_AUTH_FILE`, `KUMI_SETTINGS_FILE` | The sign-in store and the settings file |
| `KUMI_MEMORY_FILE`, `KUMI_TECHNIQUES_FILE`, `KUMI_PLAYBOOK_FILE` | Notes about you, techniques and lessons |
| `KUMI_RECIPES_DIR`, `KUMI_PROJECTS_DIR`, `KUMI_GOALS_DIR` | Recipes; each Set's conversations, notes and last state; goals and searches in progress, and background jobs Kumi closed on |
| `KUMI_REFERENCES_DIR` | Measured references |
| `KUMI_INPUT_HISTORY_FILE`, `KUMI_GAPS_FILE`, `KUMI_RESTORE_FILE` | What you sent (for ↑), the gap log, and the render tracks a listen cut off by a crash leaves behind |
| `KUMI_VIDEOS_DIR`, `KUMI_TOOLS_DIR` | Watched videos, and the programs Kumi fetches |
| `KUMI_LIBRARY_DIR` | What Kumi learned of your sounds, presets and Sets |
| `OLLAMA_HOST`, `LM_API_TOKEN` | Where Ollama listens, as Ollama reads it; LM Studio's API token, when its server wants one |
| `KUMI_EARS=0` | Hear the Set by recording, without Kumi Ears |
| `KUMI_LISTENER`, `KUMI_LISTENER_KEY` | The listening model that judges changes beside the meters, as `<base URL>#<model>` (an OpenAI-compatible server that takes audio), and its key; `off` for none |
| `GEMINI_API_KEY`, `GOOGLE_API_KEY` | A Gemini API key for the listening model (Gemini's newest Flash model, from its own list) |
| `KUMI_FAST=0` | Set device parameters through the bridge's preview and apply, the slower way ([how a change works](KUMI_CHANGES.md#how-a-change-works)) |
| `KUMI_YTDLP`, `KUMI_FFMPEG`, `KUMI_WHISPER`, `KUMI_WHISPER_MODEL` | Your own yt-dlp, ffmpeg, whisper.cpp (`whisper-cli`) or speech model (`ggml-*.bin`), by path |
| `KUMI_DAW` | `reaper` or `renoise` to work in that DAW (`ableton` for Live), as `--daw` does |
| `KUMI_REAPER_RESOURCE_DIR` | REAPER's resource folder, the only one Kumi then uses, when it isn't where REAPER keeps it by default |
| `KUMI_RENOISE_PREFS_DIR` | Renoise's preferences: a `V<x.y.z>` folder, or the folder that holds them |
| `KUMI_REMOTE_SCRIPTS_DIR` | Live's Remote Scripts folder, when Kumi doesn't find it |
| `KUMI_LIVE_EXTENSIONS_DIR` | Where `kumi bridge` puts Kumi's extension and `kumi doctor` looks for it, when Kumi doesn't find Live's Extensions folder |
| `KUMI_BRIDGE_WAIT_SECONDS` | How long `kumi bridge` waits for Live to connect; `0` doesn't wait |
| `KUMI_NO_UPDATE_CHECK` | Any value turns off the check for a newer version at start |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` | The proxy Kumi goes through, and the addresses it doesn't ([proxies](#proxies-and-school-or-office-networks)) |
| `SSL_CERT_FILE` | A file of certificates Kumi trusts besides your computer's |
| `KUMI_UI=plain` | Plain line-by-line output instead of the full-screen app; `kumi bridge`, `kumi update` and the other commands show no spinner while they work |
| `KUMI_COLOR` | `truecolor`, `256`, `16` or `none`, when detection gets it wrong; `NO_COLOR` is honoured |
| `KUMI_ICONS` | `glyphs` or `badges` (two-letter icons), when the terminal's symbols look wrong |
| `KUMI_PROBE=off` | Don't ask the terminal what it is as Kumi starts; its environment alone chooses pictures, notifications and colours |
| `KUMI_TRACE=1` | Print the name of each bridge call (no arguments or results) |

The installer reads:

- `KUMI_HOME`: install somewhere other than `~/.kumi`. Set it when installing, not afterwards.
- `KUMI_VERSION`: install a given release, such as `1.16.0`, or `latest`. Releases before 1.7.6
  install with their own script, which the installer names.
- `KUMI_RELEASES`: where to download from; it wins over `KUMI_VERSION`.
- `KUMI_NO_MODIFY_PATH=1`: leave your PATH alone.
- `KUMI_ALLOW_ROOT=1`: install for root when run with sudo. Otherwise the installer stops there, since
  Kumi would go in root's home folder, not yours.

Running it again repairs Kumi and keeps the Kumi that `kumi update --rollback` goes back to. Ctrl-C
before the new Kumi is in place changes nothing. When something goes wrong, it says why: no such
release, no connection (DNS, TLS, a proxy or a timeout), or a Wi-Fi sign-in page answering in
GitHub's place.
