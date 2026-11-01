# Kumi 指南

[English](../en/KUMI_GUIDE.md) · 简体中文 · [日本語](../ja/KUMI_GUIDE.md)

Kumi 是一个在终端里运行的制作人代理，为你在 Ableton Live 中打开的工程工作。它读取工程，按你的要求进行修改，并把每项修改显示在 HISTORY 中。它能播放、录音和渲染，聆听音频并与参考曲对比，观看视频教程，制作 Max for Live 设备，在网上查找资料，还会记住你的工作方式。它也能在 REAPER 工程和 Renoise 歌曲中工作：本指南主要讲 Live，在那里有什么不同，请见[连接 REAPER 或 Renoise](#连接-reaper-或-renoise)。

要安装 Kumi 并把它连接到 Live，请按照[快速开始](../../README.zh-CN.md#快速开始)操作。本指南介绍之后的一切。按键与界面请见[命令、按键与界面](KUMI_TUI.md)；修改与撤销如何工作，请见 [Kumi 如何修改你的工程](KUMI_CHANGES.md)。

## 登录并选择模型

在 Kumi 中，`/login` 用于登录（第一次时，Kumi 的设置会先问你）：用你的套餐登录 ChatGPT（浏览器会打开，登录结果通过 `localhost:1455` 返回），或用 API 密钥登录下表中的其他提供方，密钥粘贴到只显示圆点的输入框中。Kumi 在保存密钥之前会先向其提供方验证；如果连不上提供方，它会保存密钥，并说明尚未验证。`/logout` 用于退出登录。

在设置的登录步骤中：

- Kumi 会先检查能否连到 ChatGPT 的登录页。连不上时，它会说明原因（DNS、TLS、代理、没有及时应答或无法连接），并列出它经过的代理设置；没有设置时，会告诉你可以用 `HTTPS_PROXY` 设置。
- 按 **d** 会改用代码登录 ChatGPT，而不用浏览器：在任何设备的浏览器中输入这个代码即可。通过 SSH 时，无论是 `/login` 还是 `kumi login`，Kumi 都会自动使用代码；按 **b**（或加 `--browser`）可改回浏览器链接。
- 浏览器链接会完整显示，按 **c** 可复制。
- Ollama 或 LM Studio 运行时，还可以选择 **On this computer**。

**提供方。** 以下任意一个都可以用 `/login` 或 `kumi login <id>` 登录；`kumi auth` 会显示你登录了哪些。也可以用密钥环境变量，但用 `/login` 保存的密钥优先。

| 提供方 | 模型名称 | 密钥环境变量 | 在哪里获取密钥 |
| --- | --- | --- | --- |
| ChatGPT 套餐 | `openai-codex/<model>` | 无：`/login` 打开浏览器（用代码则按 **d**），或 `kumi login openai-codex`（没有浏览器时加 `--device`；通过 SSH 时自动使用代码，要用浏览器则加 `--browser`） | 你的 ChatGPT 套餐 |
| Anthropic API | `anthropic/<model>` | `ANTHROPIC_API_KEY` | platform.claude.com（API 密钥） |
| OpenAI API | `openai/<model>` | `OPENAI_API_KEY` | platform.openai.com/api-keys |
| OpenCode Zen 和 Go | `opencode/<model>`、`opencode-go/<model>` | `OPENCODE_API_KEY`（两者共用） | opencode.ai/auth |
| OpenRouter | `openrouter/<model>` | `OPENROUTER_API_KEY` | openrouter.ai/settings/keys |
| Venice | `venice/<model>` | `VENICE_API_KEY` | venice.ai/settings/api |
| Google Gemini | `gemini/<model>` | `GEMINI_API_KEY`, `GOOGLE_API_KEY`（也是聆听模型的密钥） | aistudio.google.com/apikey |
| DeepSeek | `deepseek/<model>` | `DEEPSEEK_API_KEY` | platform.deepseek.com/api_keys |
| xAI | `xai/<model>` | `XAI_API_KEY` | console.x.ai |
| Z.ai (GLM) | `zai/<model>` | `ZAI_API_KEY`, `ZHIPU_API_KEY` | z.ai/manage-apikey/apikey-list |
| Moonshot (Kimi) | `moonshot/<model>` | `MOONSHOT_API_KEY` | platform.kimi.ai/console/api-keys |
| Alibaba Cloud (Qwen) | `alibaba/<model>` | `DASHSCOPE_API_KEY` | modelstudio.console.alibabacloud.com |
| MiniMax | `minimax/<model>` | `MINIMAX_API_KEY` | platform.minimax.io |
| Xiaomi MiMo | `xiaomi/<model>` | `MIMO_API_KEY`, `XIAOMI_API_KEY` | platform.xiaomimimo.com |
| StepFun | `stepfun/<model>` | `STEPFUN_API_KEY`, `STEP_API_KEY` | platform.stepfun.ai/interface-key |
| Tencent TokenHub | `tencent/<model>` | `TENCENT_TOKENHUB_API_KEY` | console.tencentcloud.com/tokenhub/apikey |
| Groq | `groq/<model>` | `GROQ_API_KEY` | console.groq.com/keys |
| Cerebras | `cerebras/<model>` | `CEREBRAS_API_KEY` | cloud.cerebras.ai |
| Fireworks AI | `fireworks/<model>` | `FIREWORKS_API_KEY` | app.fireworks.ai/settings/users/api-keys |
| Together AI | `together/<model>` | `TOGETHER_API_KEY` | api.together.ai/settings/api-keys |
| DeepInfra | `deepinfra/<model>` | `DEEPINFRA_API_KEY` | deepinfra.com/dash/api_keys |
| Mistral | `mistral/<model>` | `MISTRAL_API_KEY` | console.mistral.ai |
| Ollama Cloud | `ollama-cloud/<model>` | `OLLAMA_API_KEY` | ollama.com/settings/keys |
| NVIDIA | `nvidia/<model>` | `NVIDIA_API_KEY` | build.nvidia.com |
| Hugging Face | `huggingface/<model>` | `HF_TOKEN` | huggingface.co/settings/tokens |
| Ollama（在你的电脑上） | `ollama/<model>` | 无需：运行时即可找到 | — |
| LM Studio（在你的电脑上） | `lmstudio/<model>` | 无需：其服务器运行时即可找到 | — |
| 其他服务器 | `<name>/<model>` | `settings.json` 中的 `apiKey`（[见下文](#你电脑上的模型)） | — |

- 模型名称保留提供方的 `/`（`openrouter/moonshotai/kimi-k3.1-pro`）。
- Z.ai 和 StepFun 请使用按量付费的密钥：它们的编程套餐只能用于其列出的工具。
- NVIDIA 的免费试用密钥仅供测试和评估；它的部分模型回答会延后送达，Kumi 暂时还不能使用。
- Hugging Face 需要带有 Inference Providers 权限的 fine-grained 令牌。
- Google 的 Gemini API 条款面向职业用途，在欧洲经济区、瑞士和英国需要使用其付费方案。
- `/model` 始终列出 ChatGPT、Anthropic、OpenAI 和 OpenCode，其他提供方在你登录后才会出现；其中的 **More providers** 会打开 `/login`。
- 与 Kumi 后来新增的提供方同名的自定义服务器会保留按名称生成的 ID：Kumi 会对你隐藏该提供方，`kumi doctor` 会说明如何切换。

用 `/login` 保存的密钥优先使用；没有时，Kumi 使用环境变量中的密钥，而且无法从中退出登录（请改为取消设置该变量）。API 密钥只能粘贴到 Kumi 的密钥输入框或 `kumi login` 的提示中，绝不要粘贴到消息里。Kumi 不提供 Claude 和 Gemini 的订阅登录，因为它们的提供方不允许在第三方工具中使用。OpenCode 的 Gemini 模型暂不支持。

`/model` 列出每个提供方的模型，这些模型直接从提供方读取，所以今天发布的模型无需更新 Kumi 就会出现；输入文字可以缩小列表。没有选择模型时，Kumi 使用你已登录的第一个提供方（按上表顺序）的第一个模型，并告诉你用的是哪个。`/effort` 设置模型思考的力度，可选级别取决于该模型（从 `none` 到 `max`）；越低回答越快。两者都从你的下一条消息开始生效，并保留到下次使用。更换模型会保留对话，但先前模型的私有推理除外，那部分属于该模型。

Kumi 对每个模型的了解（思考级别、能否使用工具、除文字外还接受什么）先来自提供方的列表，再来自 [models.dev](https://models.dev) 的目录，最后来自模型名称：

- 不能对话的模型（图像、嵌入、语音）不会列出。
- **chat only**：该模型不能使用工具。Kumi 可以和它谈论你的工程，但不能做任何修改，和你电脑上不能使用工具的模型一样。被要求使用工具时拒绝的模型，在 Kumi 重启前也按此处理。
- **unverified**：提供方和 models.dev 都没有描述它，Kumi 只能根据名称判断。
- **deprecated**：提供方即将停用它。

Kumi 自带一份目录副本，最多每天获取一次更新的版本；在 `settings.json` 中设置 `"modelCatalog": false` 可以停止获取。

当回答因缺少登录或登录被拒而失败时，Kumi 会提出帮你登录，然后重新发送你的消息。当提供方不提供所选模型时，Kumi 会提出让你另选一个。当你保存的模型不再出现在提供方自己的列表中（比如套餐登录不再提供它）时，Kumi 会在启动时说明，并打开 `/model`。当回答中途断开（比如连接断了）时，Kumi 会从断开处接着回答一次，并在状态行中说明；如果再次断开，Kumi 会提出重新发送你的消息。

### 上限、故障与备用模型

- **套餐上限**（ChatGPT 的五小时或每周窗口）：Kumi 等它重置，然后继续同一个回答。NOW 显示何时继续（“carrying on at 14:20”），等待的时间不计入回答的一小时，电脑保持唤醒，按 Esc 可停止。重置时间在 6 小时以后时，Kumi 会停止回答，说明何时重置，并提出改用备用模型、其他模型或登录其他提供方。
- **备用模型**代替等待继续回答：在 `/model` 中按 ctrl+f，把选中的模型设为备用模型（再按一次取消）。当你的模型达到套餐上限、额度用完，或提供方在重试后仍然故障时，它从下一步接手。它回答期间，顶栏会显示它的名字；重置后你的第一条消息起换回你的模型。没有登录的备用模型不会使用，Kumi 会说明一次。
- **繁忙或出错的提供方：** 提供方要求等多久就等多久（Retry-After，不论多长，以秒数或日期给出），每个步骤最多尝试 3 次。
- **没有网络：** 只要回答还能继续，Kumi 就一直重试，每次等得稍久一些（最多 30 秒）。
- **套餐用量：** 某个窗口用到 80% 起，顶栏显示“plan 84% · resets 14:20”。

### 你电脑上的模型

- **Ollama** 和 **LM Studio** 只要在运行就会被找到，无需登录。`/model` 会把它们各自列为一个提供方（“Ollama · on this computer”），并列出该服务器上的模型；已安装但未运行的会说明如何启动。如果你没有登录任何提供方，Kumi 会从其中能修改工程的模型开始（优先选择已加载的）。
- **其他服务器**（llama.cpp 的 `llama-server`、vLLM、Jan、网关等）无论在这台电脑上还是别处，都写在 `~/.kumi/settings.json` 中。名称就是它的 ID（`llama.cpp` 即 `llama-cpp/<model>`）。`format` 说明它接受哪种请求：`chat-completions`（OpenAI 的 Chat Completions，不写 `format` 时即为此）、`responses`（OpenAI 的 Responses）或 `anthropic`（Anthropic 的 Messages）。Kumi 对其模型的了解只来自它自己的列表，所以会标为 unverified：

  ```json
  { "modelServers": [
    { "name": "llama.cpp", "baseURL": "http://127.0.0.1:8080/v1" },
    { "name": "Studio PC", "baseURL": "http://192.168.1.20:8000/v1", "apiKey": "…" },
    { "name": "Gateway", "baseURL": "https://llm.example.com/v1", "apiKey": "…", "format": "anthropic" }
  ] }
  ```

- **给 Kumi 留出空间。** 仅 Kumi 的指令和工具就约有 2.5 万至 3 万个 token。Kumi 会向 Ollama 请求能容纳它们、对话和回答的空间（约 5.7 万个 token，以模型能读取的上限为准），并在 LM Studio 加载的模型空间不足时让它以这个空间重新加载。这些空间会占用内存，常常比模型本身还多。
- **不能使用工具的模型**仍然可以谈论工程，但无法修改它；Kumi 会说明一次，并列出同一服务器上可以修改的模型。`kumi doctor` 会列出它找到的服务器。

## 连接 Live

Kumi 通过它的桥接访问 Live：桥接由在 Live 内运行的 Remote Script 和由 Kumi 启动的本地 MCP 服务器组成。第一次打开 Kumi 时，以及每当它自带的桥接比 Live 中的新时，Kumi 的设置都会在应用内只完成尚未完成的步骤：

1. **Sign in**：如果 Kumi 还没有登录，先登录。
2. **Connect to Live**：Kumi 把桥接放到位并打开 Live。
   - **Live 没有打开时：** Kumi 先查找 Live 12。如果没有安装，或只有旧版 Live，它会说明；安装后选 **Look again** 重新检查。如果 Live 12 从未打开过，Kumi 会打开它一次，让它建立 User Library。
   - **Live 正开着时：** 选择 **Restart Live now**（Kumi 请 Live 退出，Live 会先提示你保存工作）或 **I'll quit it**（Kumi 等你自己退出 Live）。如果 Kumi 请求后过了一会儿 Live 仍然开着（例如它询问是否保存时你选了 Cancel），Kumi 会说明情况，并提出再请求一次。
   - **在 Mac 上，** macOS 第一次可能会询问是否允许终端控制 Live：请选“好”。如果你拒绝了，Kumi 会等你自己退出 Live，并告诉你在哪里允许：系统设置 › 隐私与安全性 › 自动化。
   - Kumi 关闭 Live 之后，无论这一步如何结束，Live 都会重新打开；Kumi 知道之前打开的工程时，会连同它一起打开（不知道时，工程在 File › Open Recent Set 中）。
3. **Control Surface**：第一次时，在 Live 中打开 **Settings → Link, Tempo & MIDI**，把 **AbletonMcpBridge** 选为 Control Surface。Kumi 会自己察觉，Live 也会记住这个选择。大约 45 秒后，Kumi 会补充 Settings 的位置（Mac 上在 Live 菜单，Windows 上在 Options），以及 Live 显示对话框时要先回答它。

在不能运行 Live 的 Linux 上，设置只进行登录。

在某一步按 Esc，会把这一步留到以后：Kumi 暂时在不连接 Live 的情况下聊天。Sign in 和 Connect to Live 下次会再次提出；Connect to Live 完成之后，只要 Live 应答，Kumi 就会自己连接。在放置桥接时退出 Kumi，它会等桥接放好后再退出。Live 关闭时，`kumi bridge` 在 shell 中做同样的事：它会请你确认（`--yes` 可预先确认；没有终端时，不加 `--yes` 就什么都不改），Live 正在运行时会拒绝执行，并且从不退出或启动 Live。两者都通过桥接自身的生命周期流程安装或更新桥接，带有检查、回执和回滚，并在更新之间保留它的设置和密钥。`kumi bridge` 最多等待十分钟让 Live 连接，连上时会告诉你。当 Live 中的桥接比 Kumi 自带的旧、且 Live 已关闭时，`kumi update` 会替你运行它（加 `--yes`，且不等待）。其他情况下，`kumi update` 和 `kumi doctor` 会请你运行 `kumi`：它会把桥接装入 Live，也可以替你重启 Live。

Kumi 会在你的 User Library 中找到 Live 的 Remote Scripts 文件夹，包括你移到别处的 User Library（为此它会读取 Live 自己的设置）。`KUMI_REMOTE_SCRIPTS_DIR` 可以覆盖这个位置。

**Kumi 的 Live 扩展。** 在 Live 12.4 及更高版本上，`kumi bridge` 还会把 Kumi 的扩展放进 Live 的 Extensions 文件夹（macOS 上为 `~/Library/Application Support/Ableton/Extensions/kumi.kumi`；Windows 上 Kumi 使用 `%LOCALAPPDATA%\Ableton\Extensions`，这一路径在 Windows 上尚未确认）。Live 下次打开时会启动它。这个扩展可以把 MIDI 片段直接写进编曲视图、清空轨道上的一段区域、在不播放的情况下渲染轨道的片段，并在 Live 的右键菜单（**Extensions** 下）中加入 **Ask Kumi about this**，它会把你点击的对象附加到你的下一条消息中。开启 Developer Mode（Settings → Extensions）时，Live 不会启动任何扩展，所以由桥接自己启动 Kumi 的扩展。没有这个扩展 Kumi 也能工作，`kumi doctor` 会告诉你它是否已安装、是否在运行。

桥接安装好并在 Live 中选中后，`kumi` 会找到并连接它；无需任何配置。没有桥接时，Kumi 照样启动，在不连接 Live 的情况下聊天（**No Live access**），并提出连接。`kumi --bridge-config <absolute path>` 使用你自己的桥接配置；`kumi --inference-only` 在不连接 Live 的情况下聊天（只有用 `-c` 或 `-r` 继续对话时才会保存对话）。

**Willington。** 从带有 Willington 文件的版本起，Kumi 的桥接会带上它的原生绑定；在此之前，你可以自己安装 Willington。无论哪种方式，在你用 `/willington` 开启之前它们一直是关闭的。开启后，Kumi 还可以映射机架的宏旋钮、设置链区域（chain zone），并在自检通过时编辑 Follow Actions。Willington 有绑定的 Live 版本：macOS ARM64 上的 Live 12.4.15b4 和 b5（链区域仅限 b5），以及 Windows x64 上的 Live 12.4.15b5；请见 [Willington](WILLINGTON_INTEGRATION.md)。

## 连接 REAPER 或 Renoise

Kumi 可以在 REAPER 7 及更高版本和 Renoise 3.5 及更高版本中工作，macOS、Windows 和 Linux 都支持。两者各有一个 Kumi 自己的桥接：REAPER 从 `UserPlugins` 文件夹加载的扩展（`reaper_kumi`），以及 Renoise 从偏好设置中的 `Scripts/Tools` 加载的工具（`com.kumi.Bridge.xrnx`）。Kumi 通过自己的宿主程序（`daw-mcp-server`，安装在 Kumi 旁边）与它通信，只在你的电脑内进行：桥接只在 127.0.0.1 上监听，并且只应答持有 `~/.kumi` 中密钥的 Kumi。无需选择 Control Surface。

**设置。** 运行 `kumi`，然后打开 REAPER 或 Renoise，在 Kumi 之前或之后打开都可以：Kumi 使用开着的那个 DAW。第一次时，设置会先登录，再进行 **Connect to REAPER**（或 **Connect to Renoise**），在 Linux 上也一样：Kumi 放好扩展（或工具）并打开这个 DAW。

- **DAW 正开着时：** DAW 只在启动时加载 Kumi 的桥接，而 Kumi 从不替你退出它。选择 **I'll quit it**，自己退出 DAW（它会先提示保存）；Kumi 放好桥接后会重新打开它。你保存过的工程或歌曲在 DAW 的最近文件列表中。
- **没有安装时：** Kumi 会告诉你去哪里获取（reaper.fm、renoise.com）；装好后选 **Look again** 重新检查。如果开着的是旧版 REAPER 或 Renoise，Kumi 会说明，并让它保持打开。
- **从未打开过时：** Kumi 会打开它一次，让它建立自己的文件夹；Kumi 请你退出时退出它，Kumi 会带着桥接再次打开。
- 按 **Esc** 和在 Live 中一样，会把这一步留到以后：Kumi 暂时在不连接 DAW 的情况下聊天。

`kumi --daw reaper bridge`（或 `renoise`）在 shell 中做同样的事：写入 DAW 的文件夹之前会先询问（`--yes` 可预先确认），放入桥接或把它更新到最新，DAW 开着时会提示你重启它。

**用哪个 DAW。** `kumi` 使用开着的那个 DAW，不论它是否已装有桥接。开着多个时，它选装有桥接的那个，否则选你上次用的，再否则选 Live；如果还有别的 DAW 开着，它会说明（"Renoise is open too: Kumi works with REAPER. /daw renoise switches to Renoise."）。一个都没开时，它选装有桥接的那个（装了多个时选你上次用的），并提示你打开其中一个。在 Kumi 没有连接到它的 DAW 时（或那个 DAW 还没有 Kumi 的桥接时）打开另一个 DAW，Kumi 会跟过去：只要你没在输入、也没在等回答，它会说明，然后关闭并用那个 DAW 重新打开。`/daw` 告诉你 Kumi 正在用哪个 DAW，`/daw <name>`（`live`、`reaper` 或 `renoise`）会关闭 Kumi，并用那个 DAW 重新打开。`--daw` 和 `KUMI_DAW` 总是优先，这时 Kumi 只用那个 DAW。

**Kumi 查找的位置。**

- **REAPER：** 它的资源文件夹（Mac 上为 `~/Library/Application Support/REAPER`，Windows 上为 `%APPDATA%\REAPER`，Linux 上为 `~/.config/REAPER` 或 `~/.reaper`）、常用应用文件夹中的每个便携版 REAPER（应用旁边有 `reaper.ini`）、任何开着的 REAPER（不论在哪里）、装有 Kumi 扩展的那个 REAPER，或者只用 `KUMI_REAPER_RESOURCE_DIR` 指定的文件夹。放在别处（例如另一个磁盘上）的便携版 REAPER，在它开着时就能被找到；扩展会装进开着的那个 REAPER。
- **Renoise：** 3.5 及更高版本中最新的偏好设置文件夹（例如 `~/Library/Preferences/Renoise`、`%APPDATA%\Renoise`，或 Linux 上 `~/.config/Renoise` 或 `~/.renoise` 中的 `V3.5.4`），或者 `KUMI_RENOISE_PREFS_DIR` 指定的文件夹：一个 `V<x.y.z>` 文件夹，或包含它们的文件夹。

**连接。** DAW 带着桥接运行后，Kumi 会自己连接，DAW 回来时也会再次连接。DAW 忙于对话框或渲染时，Kumi 会说明，并在它应答之前不做任何修改。

**在那里能做什么。** Kumi 读取工程或歌曲，并带撤销地修改它（能修改什么、撤销在那里如何工作，见 [REAPER 与 Renoise](KUMI_CHANGES.md#reaper-与-renoise)）；FOCUS、NOW 和 HISTORY 跟随 DAW 自己的视图（[命令、按键与界面](KUMI_TUI.md#reaper-与-renoise)）。它会聆听、判定自己的修改、配合 `/loop` 和 `/goal` 工作、匹配参考曲、执行即时短语、告诉你它关闭期间的变化、把你手动做的操作变成配方，并使用你的素材库、笔记和对话，全部用这个 DAW 的说法（各 DAW 的对照见 [Kumi 在 Live、REAPER 和 Renoise 中能做什么](DAWS.md)）。在 REAPER 中它还能运行 REAPER 的动作（action）、API 和 Lua（ReaScript）；在 Renoise 中能运行 Lua。以下仍只属于 Live：Max for Live 设备、Kumi Ears、Willington、Kumi 的 Live 扩展（含 **Ask Kumi about this**）、Live 自己的命令和 Live 的手册。

**版本。** 每个 Kumi 版本都附带一个扩展和一个工具，`kumi update` 会把它们更新到最新（见下文“更新、报告与卸载”）。
Kumi 1.24.35 至 1.24.40 附带 REAPER 扩展 0.1.8，1.24.10 至 1.24.34 附带 0.1.7，1.24.0 至 1.24.9 附带 0.1.6，1.23.3 附带 0.1.5，1.23.2 附带 0.1.4，1.23.1 附带 0.1.3，1.21.2 至 1.23.0 附带 0.1.2，1.21.0 至 1.21.1 附带 0.1.1，1.20.0 附带 0.1.0。
Kumi 1.24.35 至 1.24.40 附带 Renoise 工具 1.0.7，1.24.10 至 1.24.34 附带 1.0.6，1.24.0 至 1.24.9 附带 1.0.5，1.23.3 附带 1.0.4，1.23.2 附带 1.0.3，1.23.1 附带 1.0.2，1.21.0 至 1.23.0 附带 1.0.1，1.20.0 附带 1.0.0。

## 使用 Kumi

Kumi 全屏运行：左边是对话，右边是 Live 面板，底部是输入框。FOCUS 跟随你在 Live 中触碰的对象，NOW 显示 Kumi 正在做什么，HISTORY 列出每项修改及其撤销。窗口宽度不足 100 列时，面板会变成对话下方的抽屉，Kumi 每做一次修改它就会打开，Ctrl-\\ 可打开或关闭它。设置 `KUMI_UI=plain`，或者把输出通过管道传给其他程序，会改为逐行的纯文本输出，适合屏幕阅读器。详情请见[命令、按键与界面](KUMI_TUI.md)。

试试“描述打开的工程：轨道、速度和走带”，然后问某条轨道上有哪些设备。再请求一项修改，比如“把速度设为 124，并把 3-Audio 重命名为 Bass”：每项修改都会出现在 HISTORY 中，旁边带有 **undo**。点击 FOCUS 中的某个设备，或在 Live 中右键点击某个对象并选择 **Ask Kumi about this**，即可指向它：“这个 Saturator 太刺耳了”。

Kumi 工作时，按 Enter 可以补充说明（它会在当前这一步之后读到你的消息），按 Tab 发送一条等这次回答结束后再处理的消息，`/btw` 可以顺便问个问题而不打断它。Esc 停止这次回答；已完成的步骤会保留。`/stop` 随时停止 Live（片段、走带和录音）。

当 Kumi 请你做选择时（哪条轨道、哪个版本），选项会显示在输入框上方：按选项的数字再按 Enter 回答，或者直接输入你自己的回答。

要给 Kumi 看东西，把文件拖进窗口，或按 **Ctrl-V** 粘贴剪贴板里的图片（比如某个合成器的截图），然后说“做一个这样的”。每个文件都会显示在输入框上方，随下一条消息一起发送；点 ×，或在输入框为空时按 Backspace，即可撤回。模型能看的图片是 PNG、JPEG、GIF 和 WebP，每张不超过 3.75 MB，一条消息合计不超过 20 MB。其他文件，比如参考音频、预设或 Live 工程，会以路径的形式发送，供 Kumi 使用。图片只随它自己的那条消息发送；之后的对话（无论是否保存）会保留每个文件的名称和路径，但不保留图片本身。剪贴板中的图片在 `~/.kumi/attachments` 中保存一周。

## 后台任务

不改动工程的耗时工作，Kumi 会放到后台去做，这期间你可以继续和 Kumi 一起工作：网上查资料、看教程视频、查 Live 手册、翻你的素材库或之前的对话，或者写出一个 Max for Live 设备的文件。

- **开始：** Kumi 用一句话说明正在运行什么，随即就能接收你的下一条消息。你可以主动要求（“在后台查一下这个”），也可以交给 Kumi 决定。
- **查看：** 顶栏显示正在运行的任务数（◐ 2 jobs），**JOBS** 标签页列出每个任务正在做什么、用时和 token。`/jobs` 也能列出它们。
- **结束时：** 对话中会出现一张卡片，显示它的第一行（“✓ Job 2 · Burial vocal chops · 5 steps, 3 sources”），Kumi 会在你下一条消息时读取它的报告。你的请求所需要的任务（“看看这个，把贝斯做成他们那样”）会在你没有在选择、输入或打开面板时自己接着进行。
- **Live 只由 Kumi 操作：** 任务只负责查找和撰写，从不读取或改动 Live。修改由 Kumi 自己完成，记在 HISTORY 中并可撤销。任务读到的内容是数据，不是指令。
- **插件预设：** 后台任务可使用内置 VST3 工具创建和编辑受支持的预设文件，遵守与主对话相同的版本和写入检查。任务返回文件路径，由主对话在 DAW 中加载、回读参数并试听。适用于 Live、REAPER 和 Renoise。
- **Ctrl-B** 把回答中不用 Live 的耗时步骤（视频、网页）移到后台，回答继续进行。在 tmux 中按两次 Ctrl-B。
- **停止：** 在 JOBS 的行上或任务详情中按 `x`。Esc 只停止回答，不停止任务。退出 Kumi 会中断任务，下次可用 `/jobs` 重新运行。
- **离开时：** Kumi 的窗口在其他窗口后面时，任务结束会以通知和 Live 状态栏中的一行告诉你；任务运行期间电脑不会休眠。
- **设置：** `maxJobs`（同时运行三个）和 `workerModel`（任务使用的模型，从 `/model` 的列表中选；默认与对话相同）。每个任务的 token 显示在它那一行。

## 对 Kumi 说话

按下 **Ctrl-T** 说出你想要的，然后再按一次。你说的话会出现在输入框中；按 Enter 发送。

- **按住说话。**按住 Ctrl-T 说话，松开即停止。支持 kitty 键盘协议的终端（kitty、Ghostty、WezTerm、iTerm2）会告知按键何时松开；在其他终端中，Kumi 依据按键自身的重复来判断。
- **Kumi 聆听时**，输入框会显示一个闪烁的圆点、时间和电平表。Enter 立即停止并发送；Esc 放弃。你说完后安静 3 秒会自动停止。每次最长两分钟。
- **`/voice`**：开始或停止；停止后不按 Enter 直接发送；你说的语言（英语、电脑的语言或任意语言）；麦克风。
- **隐私。**ffmpeg 收听麦克风，whisper.cpp 在你的电脑上写下你说的话；转写完成后录音会立即删除。
- **需要的东西：**ffmpeg 和 whisper.cpp（在 Mac 上运行 `brew install ffmpeg whisper-cpp`；在 Windows 和 Linux 上由 Kumi 下载），以及 Kumi 在你第一次说话时下载的语音模型（约 190 MB，与观看视频共用），外加一个让音乐和噪声不被当成文字的小型语音活动检测模型。`kumi doctor` 会告诉你说话功能是否就绪。
- **第一次在 Mac 上使用时**，macOS 会询问是否允许你的终端使用麦克风。Kumi 听不到你时会说明原因（没有权限、只有静音、只有很小的声音、听不出文字），并给出解决办法：隐私设置，或换一个麦克风。
- 说话功能只在全屏应用中可用；纯文本模式（`KUMI_UI=plain`）只能打字。

## 修改与撤销

Kumi 把一个请求规划成一组修改，并一次性执行。每项修改都会带着一个通俗的标题（“Tempo 120 → 124 BPM”）进入 HISTORY。点击旁边的 **undo**，输入 `/undo` 撤销最近的一项，或者直接让 Kumi 撤销。撤销会准确恢复该修改所替换的内容。无法撤销时（对象已不存在，或者对某些设置而言，你之后又改了同一处），该行会显示 **kept** 并说明原因。整个计划在 Live 自己的撤销中也只是一步，所以在 Live 中按一次 Cmd-Z（Windows 上为 Ctrl-Z）就能撤回。

有些修改 Kumi 无法撤回（删除轨道、裁剪片段、添加机架链）。HISTORY 会把它们标为 **kept**，而 Live 自己的撤销仍然可以撤回它们。Kumi 会在你要求时删除内容，或在请求隐含删除时这样做（“从头再来”“换掉鼓”）。

**大改动前先备份。** 当一个计划进行到第三步，或进行到会删除内容的步骤时，Kumi 会把最后一次保存的工程复制一份，放在它旁边（`Song.backup-<date>.als`），并告诉你；每个已保存的版本只复制一次。未保存的工作不在文件中，所以也不在副本中；从未保存过的工程不会有副本。

**在 Live 中运行 Python。** 对于其他工具够不着的地方，Kumi 可以用 Live 自己的 API 在 Live 内运行 Python。脚本所做的修改在 Live 的撤销中算一步，但不会在 HISTORY 中留下条目；用 Live 的撤销即可撤回。

**Live 不允许脚本做的事：** 把宏旋钮或调制器映射到参数（Willington 两者都能映射：`/willington`），以及编辑编曲视图的自动化通道。Kumi 会如实说明，并建议变通的办法。保存、导出、冻结、并轨和编组则通过 [Live 自己的命令](#live-自己的命令)完成。

[Kumi 如何修改你的工程](KUMI_CHANGES.md)列出了 Kumi 能做的所有修改。

## 播放、录音与渲染

Kumi 会在你要求时，或者在有助于检查或展示它做出的东西时，播放和停止工程、触发片段和场景、移动播放头以及录音。如果一个计划中途停止（某一步失败，或你按了 Esc），Kumi 会停止它启动的播放和录音。如果 Live 拒绝普通的停止，Kumi 会使用桥接的紧急停止。

Live 不给脚本提供并轨（bounce）功能，所以 Kumi 通过重采样来并轨：它添加一条音频轨道，从源轨道取信号（整个混音则从 “Resampling” 取信号），在编曲视图中录下你要的长度，然后解除该轨道的录音准备。录音会作为音频片段留在工程中。有了扩展，Kumi 还可以在不播放的情况下，把音频轨道自身的片段渲染成文件（取轨道设备之前的信号）。Live 自己的 Bounce to New Track 和 Bounce Track in Place 也可以使用，通过 [Live 自己的命令](#live-自己的命令)。

## Live 自己的命令

有些事 Live 的脚本接口完全没有提供。对于这些，Kumi 会像你一样使用 Live 自己的菜单：编组和取消编组轨道；冻结、解冻和平铺（flatten）；不播放即可并轨（Bounce to New Track、Bounce Track in Place）；合并（Consolidate）；把音频转换成 MIDI（旋律、和声、鼓）；分离音轨（stems）；切片到 MIDI 轨道；保存工程，或收集全部并保存；导出音频或 MIDI 片段。

Kumi 也通过 Live 的 File 菜单处理工程文件：它可以新建工程；在 Windows 上，还能按你给的名字和文件夹另存工程（Save Live Set As），或按文件打开一个工程。在 Mac 上 Kumi 还不能填写 Live 的保存和打开对话框，所以这两件事它会请你在 Live 里完成。如果当前工程有未保存的改动，Live 会先询问；Kumi 按你说的保存或放弃，你没说时就问你。如果要保存的位置已经有文件，Live 会在替换前询问，Kumi 把这个回答留给你。Live 把工程保存在项目文件夹里：以新名字保存到不是项目的文件夹时，它会在那里建一个“<名字> Project”，Kumi 会告诉你工程保存到了哪里。打开工程期间 Live 会暂时断开与 Kumi 的连接，而要求打开它的那个请求会在新工程里继续。Kumi 从不自己写 `.als` 文件。

- Kumi 会选中命令要作用的对象，按下命令，并说明发生了什么变化。Live 打开对话框时（比如导出），Kumi 会读取它（包括其中的复选框）并回答它。
- 分离音轨时，Kumi 会在 Live 的对话框里勾选你要的音轨（人声、鼓、贝斯、其他；你没说时四个全选），你要求时把两三个合并到一条轨道上，并在你指定时选 High Quality 或 High Speed（没指定时沿用你上次用的）。然后它按下 Separate，等 Live 处理完：整首歌要一分钟或更久。
- 轨道是通过 Live 12 为屏幕阅读器提供的辅助功能按名称选中的。Live 保持原样：不会有任何窗口跳到前面，一条命令用不了一秒（并轨或冻结则取决于 Live 渲染所需的时间）。
- 已经冻结的轨道不会被再次冻结：Live 的命令会把它撤回，所以 Kumi 会先检查。
- 片段命令作用于 Session 中的片段，或你在 Live 中选中的片段。Kumi 目前还不能选中编曲视图中的片段。
- HISTORY 会列出每条命令，Live 自己的撤销（Cmd-Z）可以撤回它。
- **在 Mac 上**这使用辅助功能。第一次时 macOS 会询问：请在 系统设置 › 隐私与安全性 › 辅助功能 中打开运行 Kumi 的应用（你的终端）。`kumi doctor` 会告诉你它是否已打开。**在 Windows 上**它通过 Windows 本身读取 Live 的菜单栏和对话框（选中轨道则用 UI 自动化），无需任何设置；`kumi doctor` 会检查它能否读取 Live 的菜单。在 Windows 上，Live 的确认对话框用 Yes、No、Cancel 代替 Mac 上的 Save 和 Don't Save：两种说法 Kumi 都接受。

## 聆听

Kumi 能听音频文件和工程中的音频片段：参考曲、采样、并轨结果或录音。它会测量：

- 响度：整合 LUFS、真峰值和响度范围；
- 十个频段的音色平衡（从超低频到空气感频段），以及每个频段的立体声宽度；
- 动态、速度和调性；
- 对单个声音：它的音高、泛音、包络和运动（LFO 的速率，按速度换算）；
- 对带音符的声部：音符本身（取自前一分钟），可以写成 MIDI 片段。

给出参考曲时，它会对齐响度，并说明差别最大的地方。对话中会以小型频谱显示它听到的内容，对比结果则显示为比参考曲高或低多少 dB。Kumi 自己就能读取 WAV 和 AIFF，MP3、M4A、FLAC 等格式则通过 macOS 的 `afconvert` 读取，在其他系统上通过 `ffmpeg` 读取（在 Windows 上，Kumi 会在第一次需要时下载它）。分析在你的电脑上进行：只有数字会发送给模型，音频本身绝不会。

**聆听工程。** 询问某条轨道或混音（“贝斯是不是太浑？”“什么和底鼓打架？”），Kumi 会直接在 Live 中聆听，无需任何设置：

- **Live 正在播放时**，它聆听正在播放的内容几秒钟，不动走带。听整首歌时，即使在播放中也会从头播放来听，并让 Live 保持停止。
- **Live 停止时**，它播放循环区（或从播放头起的几个小节、你指定的部分，或直到最后一个片段的整首歌）来听。听多久，就是所听音乐的长度。
- **Kumi 听到的，你也听得到。** Kumi 聆听时从不把 Main 调低，也不把正在处理的轨道静音，所以你可以一起听并随时引导它。每次聆听都会说明播放哪些小节、为了听什么（“在 300 Hz 处衰减之后”）。
- **同时聆听多条轨道：**每条的声音，以及两条轨道在同一频段以相近电平重叠的地方。
- **原理：**Kumi Ears，一个 Kumi 自带的小型 Max for Live 设备（`kumi bridge` 会把它放进你 User Library 的 Kumi 文件夹）。Kumi 需要聆听时把它放在轨道设备链的末端，用完后移走。除了它挡住的候选（见下文），声音原样通过，工程中不会录下任何东西。
- 试听（audition）和声音匹配搜索也用同样的方式聆听候选：不需要临时轨道，也不需要预备录音。试听时你听到的是第一个候选，其余候选在 Kumi 录音时由 Kumi Ears 挡住。搜索会挡住它的所有候选，并在暂停或结束时把最好的那个放到“Kumi · Goal best”上。没有 Max for Live 时，Kumi 会改为先录音再聆听，候选会一起发声。

**聆听 REAPER 工程或 Renoise 歌曲。** Kumi 会离线渲染那一段（比播放快，并在前面多渲染一小节），听完后再大声播放给你听（最长一分钟）。单独的一条轨道会单独渲染：REAPER 用推子前的分轨（stem），Renoise 用轨道独奏。

- **REAPER 播放时，** Kumi 可以直接听正在播放的主输出（默认 8 秒）；单独的轨道则渲染。
- **Renoise 播放时，** 渲染会让它停止播放，而 Renoise 的脚本听不到它正在播放的声音，所以 Kumi 只渲染你指定的那一段。
- **判定修改时，** REAPER 用最后一个 ReaLimit（没有则用 Kumi 添加的 JS: Volume Adjustment）保持电平，Renoise 用最后一个 Maximizer（没有则用 Gainer）。目标和声音匹配搜索在轨道的临时副本（“Kumi · try …”）上尝试候选，结束后移除；最好的结果留在“Kumi · Goal best”上。
- Renoise 的试用版（demo）无法渲染，所以 Kumi 在那里不能聆听；`kumi doctor` 会说明。

## 判定修改与循环

对于可以测量的工作（按响度做母带、压住刺耳的共振、让人声突出），Kumi 会判定自己的修改，而不是想当然：

- **目标变成一张检查清单：** 响度和上限、平衡、参考曲的轮廓，或必须突出的元素，每一项都有容差。Kumi 先听整段，列出问题：刺耳或有共振的频率、过宽的低频、隆隆声、直流偏移、过响的贝斯音、过载和削波，以及遮蔽你指定元素的声音。
- **遮蔽在 Main 的效果链之前测：** Kumi 在 Main 的效果链之前听你指定的元素。有 Max for Live 时，它也听进入 Main 效果链时的混音（放在 Main 最前面的第二个 Kumi Ears，每次听完就移除），所以无论 Main 上有什么，遮蔽都能公平地测：那里的限制器或 EQ 不会改变结果。没有 Max for Live 时，它只能听 Main 效果链之后的混音，所以只有当 Main 上的设备关闭、是表（Spectrum、Tuner），或是只调电平的 Utility（元素也按那个电平来听）时，遮蔽才在检查清单上。Main 上有其他设备时就不测，运行会说明：要让元素突出，先关掉 Main 上的设备来判定，之后再单独判定母线的响度和峰值。
- **一次只改一处：** 每次修改后，Kumi 用轨道上最后一个 Live 的 Limiter（没有就用它加上的 Utility）把电平拉回应在的位置（目标响度，或原来的电平），所以修改前后是在相同响度下判定的。只有目标项变好、且其他方面没有听得出的变差时才保留修改（再也测不出的项目，比如变成静音，算作变差）；如果在这个电平下峰值又回来了或出现削波，这处修改也会撤回。不保留的修改，用 Kumi 自己的撤销撤回。
- **数值由代码来选。** 改什么由 Kumi 决定，具体数值由代码以尽量少的试听找出：根据测量结果计算 EQ（压住共振所需的最小衰减，或向参考曲轮廓靠拢的频段）、把一个旋钮逐步逼近目标（调限制器增益直到响度合适、调去齿音器的阈值），在一个旋钮的整个范围内探测（七个设置在轨道的临时副本上并排、一次听完，再把它设在反应达到目标的位置），或者把几个相互影响的旋钮按小规模的代数搜索，每一代都用同样的方式来听。探测和搜索需要在一条轨道上判定的运行（`judge` 带 `track`），不能用于整个混音。一个旋钮在某条轨道上起了什么作用会记住一个月，下次在那里逐步逼近它时就从那里开始。每增加一个设备、每一处改动，都必须物有所值。
- **轮次记录**把每一轮显示在对话里：目标、修改、修改前 → 后的数值、保留还是撤回及原因、下一步，以及到目前为止的试听次数。连续两次修改都无效的项目会先放一放，Kumi 转去处理下一个。
- **聆听模型**（如果有）也会听修改前后，听的是单声道（所以不会问它声场宽度或高频空气感）。如果它听出失真、抽吸（pumping）或削波，并且两种顺序下都更喜欢修改前的那一版，就可以否决这处修改；其余判断由仪表决定。有 Gemini API 密钥（`GEMINI_API_KEY`）时使用从 Gemini 自己的列表中选出的最新 Flash 模型；否则有 OpenAI API 密钥时使用 OpenAI 最新的音频模型；`KUMI_LISTENER` 可以指定其他模型（`<base URL>#<model>`，例如你电脑上的模型服务器），`KUMI_LISTENER=off`（或任何不是模型地址的值）则只用仪表。每次运行的第一轮会说明由哪个模型来听。
- **模型槽位。**`/slots` 显示每项聆听工作由哪个模型负责：分轨（Live 自己的分轨）、扒谱（Live 的转换）、聆听（上面的查找顺序）和嵌入（Kumi 自己的：风格用 LAION-CLAP 的音乐模型，效果用 AFx-Rep）。可以用平常的话替换，例如 `/slots listening gemini`、`/slots listening off`（立即生效，运行中也一样）或 `/slots listening http://127.0.0.1:8080/v1#<model>`。新的聆听模型会先在一段已知的片段上测试，只有听对了才切换。`/slots back <job>` 撤回替换。嵌入在 Kumi 自己的模型运行时里运行：`/slots embeddings ~/models/clap.onnx`（含空格的路径请加引号）或指向任意主机上 `.onnx` 文件的 https 链接（最大 4 GB，按 Esc 可停止下载）会先用两个已知的音测试，能分辨它们才会使用；它会取代风格模型，效果模型仍是 Kumi 自己的。之后文件若不见了，会改用 Kumi 自己的模型，并只提示一次。`/slots embeddings off` 关闭嵌入。分轨和扒谱暂时还不接受模型文件。Kumi 读不了的 `slots.json` 会让聆听保持关闭。设置了 `KUMI_LISTENER` 时，它优先于聆听槽位，但 `/slots listening off` 仍能关闭它。
- **由模型来听的风格和效果。**有参考曲时，Kumi 的风格模型（LAION-CLAP）会听参考曲以及这次运行的每次聆听，让声音离参考曲的风格更远的修改会被撤回。对声音目标，效果模型（AFx-Rep）也对声音的效果做同样的事。它们在第一次需要、且磁盘空间足够时下载一次到 Kumi 文件夹里的 `models`（`~/.kumi/models`，或 `KUMI_HOME` 下）（ONNX Runtime 约 10 MB，Windows 上约 80 MB；模型约 245 MB，每个都用校验和验证）。

**参考**也会变成目标。给 Kumi 一个文件、一个装有文件的文件夹、YouTube 视频或播放列表、Spotify 链接，或者只是一句话（“像 Burial”“dub techno”“Kid A”）：它会找到示例曲目（艺人的代表作、专辑里的曲目、某个流派的主要艺人），逐一测量（跳过无声或低于 −45 LUFS 的安静曲目，以及文件夹或搜索中不到 30 秒的小样），并保存一份带有这些曲目所处范围的轮廓，“在这种风格里”就是指在这个范围之内。话可能有多种意思时（同名的两位艺人，或同名的专辑和艺人），Kumi 只问你一次指的是哪一个，你的回答就能确定。每个参考只测一次，保存在 `~/.kumi/references`；文件或文件夹里的文件有变化时会重新测量。对于参考中的某个元素（它的贝斯、它的鼓），Kumi 可以把曲目放进你的工程，用 Live 自带的分轨功能分离出分轨（Kumi 可能会请你点一下这条轨道的片段），再测量其中一轨。

**一个声部的律动**在音符上判定，不用试听：在小节里的位置（Drum Rack 上的声部分五条线：底鼓、军鼓、踩镲、镲片和打击乐）、每一步提前或延后多少（推进和摇摆）、重音、切分和加花，与参考的 MIDI 比较。对于一张唱片的鼓，Kumi 先分离出分轨（Kumi 请你时，点一下这条轨道的片段），用 Live 的 Drums to MIDI 把鼓分轨转成 MIDI，并把每一下击打对准分轨里真正的起音。不在 Live 里的有音高的声部（比如音频文件里的贝斯线，最多取前 2 分钟）会用 Basic Pitch 转成音符，放在按这些音符拟合出的网格上（速度和第一个强拍）；来自音频时，比较的是时值和摇摆，不比较重音。加花需要 8 小节或更多。Kumi 也可以自己把你的音符一步步移向参考，只保留更接近、且别的方面没有走远的修改。

**一首歌的结构**按小节来听：能量曲线（响度、密度、明亮度、低频）、各段落的角色与重复、段落之间的过渡以及每次 drop 的铺垫、前奏、尾奏和第一个 hook，以及哪些音轨在哪里出现（最多 64 条音轨）。Kumi 也会指出不对的地方：缺少对比的段落、能量停滞、零碎的乐句长度、没有铺垫就来的 drop、32 小节以上没有变化的循环。有参考曲时，它会说明两者结构的差别。

**一个声音**会和其他音轨一起，在一次渲染中测量：起音、相对于拍子的衰减、明亮度、808 或底鼓的音高下滑、调制的速度（几拍一个周期）、温暖度（二次和三次谐波）、宽度、高频上限、底噪、尾音拖多长，以及噼啪声。Kumi 会指出音符边缘的咔哒声、直流偏移和突出的音符。一套鼓组会被当作整体判定：底噪、高频或颗粒感与其他不同的鼓件，相互堆叠或留有空隙的鼓件，一直响到下一下击打的衰减。相对于底鼓，它会说明每个声部被压低多少、多快恢复，以及击打是错开还是相撞；告诉它调性时，它会说明贝斯线的音符离调有多远。效果也从听到的声音里读出：混响的衰减时间以及尾音变暗、变宽的程度，延迟回声的音符时值、重复次数和每次衰减多少，电平摆动的深度，滤波器扫动的范围，相对于拍子的泵感，以及逐小节尾音所占的比例。Kumi 也会指出被截断的尾音、不合拍的回声、消不掉的重复，以及盖过干声击打的尾音。让 Kumi 把声音或效果做得像参考，它会逐轮缩小测得的差距，只保留更接近的修改。

`/loop` 加上要达到的目标，会按轮次工作直到达成：一次修改，判定，然后下一个目标。何时停止由 Kumi 决定，而不是模型：所有项目都在容差内、修改不再有帮助，或经过 16 轮或 45 分钟。检查清单保持最初设定不变，只有 Kumi 要求时模型才结束判定：再听一遍整段，并说明改了什么。模型自己判定一次修改时，Kumi 会自己开始同样的循环（只测量不会开始）。按 Esc 或 `/loop stop` 停止；你发来的消息在得到回答后也会结束它。已保留的修改不变。

`/goal` 加上要达到的目标，会跨多个回合坚持一个目标直到达成。每个回合之后 Kumi 都会检查：测量过时看判定的数值，否则由模型自己检查，且必须回答完成、受阻（以及你需要先做什么）或继续（以及下一步）。到 12 个回合、一小时，或连续三个回合没有更接近时停止（按判定的数值是缩小一步或更多，否则是工程有了改变），并说明进展到哪里。需要你来解决的错误（登录、账单、模型、设置、Live、6 小时后才重置的套餐上限）会让它以受阻停止，并说明该怎么做；更近的上限会等过去，或由备用模型继续。当一个回合以带有 Kumi 推荐选项的问题结束而没人回答时，一分钟后（`answerAfter`）Kumi 会选它，目标继续（`/goal answer: <words>` 可手动做同样的事）。运行中你发来的消息会先得到回答，然后目标等你。GOAL 标签页显示目标、回合数和用时与预算的对比、最后一次检查以及下一步。单独输入 `/goal` 显示它；`/goal edit <words>` 修改它，`/goal resume` 继续（用完的部分换上新的预算），`/goal pause` 或 Esc 暂停，`/goal clear` 结束（没有未完成的目标时，结束暂停中的声音匹配搜索）。已有未完成的目标时再发另一个 `/goal`，会先确认：再发一次或用 `/goal new: <words>` 来替换。目标随工程保存，重启后仍在。未保存工程的目标在工程打开期间（即使 Kumi 重启或开始新对话）按 Live 对这个工程的标识保存，并在工程第一次保存时随之移动。

## 匹配参考曲

让 Kumi 把某个东西做得像参考曲（“让贝斯听起来像这个：~/refs/bass.wav”），它会把这当作一次搜索，而不是猜测。它先聆听参考曲，在各自的轨道上搭建两到四个不同的版本，然后把它们一起渲染，逐一与参考曲对比打分（0 到 100 分，并列出最大的差别）；你听到的是第一个版本，其余的只有 Kumi 听。接着它精修最好的那个；当没有哪个旋钮能缩小差距时，它会改变结构；在达到目标、新想法不再有帮助，或经过 12 轮或 45 分钟后停止。最后它会给出前后的分数，以及仍然存在的差别。

`/loop` 加上要匹配的声音会走得更远：Kumi 会一直搜索，大部分时候用自己的快速旋钮搜索，每隔几代再加入模型更大胆的想法，直到分数达到 95、你让它停下，或过了四个小时。GOAL 标签页显示进展。有 Max for Live 时，Kumi 在尝试候选时会把它们挡住，所以你听到的是不含这些候选的工程；搜索暂停或结束时，最好的那个会放到“Kumi · Goal best”上。Esc 暂停搜索，单独输入 `/loop` 会继续它（即使重启之后也可以），`/loop stop` 结束它。

Kumi 从每次匹配中学到的东西会作为一条经验（✦）保留下来，供下一次使用；`/memory` 会列出它们。

## 编曲

可以让 Kumi 把一个循环做成一首曲子：“把这个编排一下”“用这些场景做一个 3 分钟的编曲”，或者附上文件说“照这首参考曲编排”。

- **素材。**Session 的场景（一个段落播放某个场景的片段，或按轨道选定的片段），或编曲视图中已有的小节（其中的 MIDI 片段）。
- **结构。**带名称和小节数的段落。你没给出时，Kumi 会挑一个适合曲风和速度的结构。有参考曲时，它会听出参考曲的结构（各段落、它们的能量、哪些会重复出现）并照着做。
- **变化。**轨道逐段进出。过渡包括 drop 之前的空白、段落末尾某条轨道的 fill 片段，以及在下一段开头结束的 riser 或镲片（来自效果轨道的片段或你的采样）。除非你要求，Kumi 不会写新的声部。
- **在 Live 中。**你的片段会被复制到编曲视图中已有内容之后（或你指定的位置），每个段落都有一个定位器，播放头回到开头。
- **撤销。**整个编曲在 HISTORY 中是一行、一次撤销，在 Live 中也是一次 Cmd-Z。
- **限制。**Live 的脚本接口无法在编曲视图中绘制自动化，所以滤波扫频和音量起伏需要你自己画。编曲视图中已有的音频片段无法在那里复制：请先把它们拖到 Session 的槽位中。Live 播放时，定位器会等待。

参见[编曲的原理](KUMI_CHANGES.md#编曲)。

## 插件

- **内置 VST3 预设工具。** 请求创建或修改预设时，Kumi 会先检查本地格式目录，再考虑外部资源。支持的预设可通过带历史记录和撤销的操作加载。Serum 2、Ozone 12 及其独立模块、Valhalla、Minimal Audio、Pulsar Modular 等的版本和限制见[格式说明](../../plugin-formats/README.md)。
- **优先使用已安装的处理器。** 母带和参考母带处理先检查 Ozone 12，再检查其他合适的 VST3，最后使用 DAW 原生设备。Kumi 会在修改前后测量和试听，并在匹配响度后比较参考音频。
- **FabFilter 预设。** 对已验证的安装版本，Kumi 可创建、查看和修改 14 款现售产品及 11 款旧产品的 VST3 预设，并通过 Live、REAPER 和 Renoise 的预设流程加载。旧版本使用各自采集的格式；未验证的版本保持只读。保存值的尺度可能与旋钮显示值不同，目录中会说明。
- **导入 FabFilter 预设。** 提供本地 `.ffp` 文件，Kumi 可将它转换或编辑成可加载的 `.vstpreset`。未指定新名称时使用文件名作为预设名；不支持的格式或数值会被拒绝。
- **原生文件的限制。** Softube 和部分 Zynaptiq 预设支持查看、复制模板和重命名；未验证的数值参数保持只读。未验证产品的 VST3 状态加载不会被视为可用。Live 从浏览器加载新的预设实例；现有实例通过公开参数修改。
- **经验证的写入。** 已安装版本必须与经过测试的写入器匹配，数值修改必须有经验证的写入范围；文本和布尔值修改必须有经验证的含义或明确的允许列表，资源路径保持只读。新文件保存在 Kumi 的预设文件夹中。Live 还会在 User Library 保存副本；撤销已加载设备时，该文件仍会保留。

Serum 2、Vital、Ozone 12、Pro-Q 4、Pro-L 2、Saturn 2、Decapitator、OTT、Supermassive 和 Pigments 还有详细指南。处理某个插件之前，Kumi 会先读这个插件的指南：它的用途、各个部分、常见声音的配方，以及与插件向 Live 显示的真实参数的对照。

- **用插件自己的单位设定数值：**“800 Hz”“-6 dB”“35 %”，或按名称选菜单项（“Saw”），按插件自己的显示来设定。
- **Live 显示的参数。**Live 只让 Kumi 调节插件中已配置的参数。指南会说明是哪些，以及如何添加更多：点击插件标题栏中的 **Configure**，在它的窗口中把旋钮动一次。Kumi 可以帮你打开插件的窗口。
- **按作用指定旋钮。**Kumi 调节插件时，可以按作用来指定旋钮：指南中的角色（Ozone 12 的 ceiling），或任何设备都有的工作（限制器的增益、它的上限、EQ 频段的增益、压缩器的阈值）。它会在插件向 Live 显示的名称中找到这个旋钮，只有在已配置时才调节它。如果没有配置，Kumi 会说明要配置哪个旋钮，同时让同一轨道上的另一个设备来完成这项工作：先是 Ozone 12，然后是 Kumi 了解的其他插件，最后是 Live 自己的设备。EQ Eight 上的 EQ 频段，是离这处修改所针对的频率约一个八度以内、已开启的频段（增益则须是钟形或搁架）中最近的那个；没有时 Kumi 会问是哪个频段。轨道上一个都没有时，Kumi 会先检查合适的已安装 VST3，再考虑 Live 原生设备：限制器放在最后，EQ 或压缩器放在最后一个限制器之前。
- **未公开的参数。** Kumi 先检查内置预设格式是否支持所需设置。格式范围之外的操作以及 Ozone 的 Master Assistant 等交互功能仍需使用插件窗口；指南会说明在哪里。
- **波表。**Kumi 可以用波形和谐波，或从一段声音中截取，为 Serum、Vital 等波表合成器制作波表，放进插件的文件夹；你把它拖到振荡器上即可。

其他插件也能按参数名称使用。

## 观看视频教程

给 Kumi 一个视频，让它搭建视频里展示的内容。视频可以是 YouTube 教程（或任何 [yt-dlp](https://github.com/yt-dlp/yt-dlp) 能读取的网站上的视频），也可以是你电脑上的视频文件：

> 看看这个，在新轨道上做出这个贝斯：https://www.youtube.com/watch?v=…

Kumi 会读取视频的标题、章节和文字内容。文字来自视频的字幕；没有字幕时，则来自视频里的语音，在你的电脑上转写。然后它会查看旁白提到某个设备、设置或数值时的画面，需要读取数值时还会放大查看。接着它用几行话说明这个视频做了什么，并在你的工程中搭建出来。视频用到了你的工程里没有的东西时（某个插件、某个采样），Kumi 会说明，并改用 Live 中最接近的设备。

在一次回答中，Kumi 已经看过三次的画面不会再给它看，同一个视频最多看 240 张画面。像“列出它用到的所有设置”这样的长请求，Kumi 会用已读到的内容作答，并说明哪些没能读到，而不是反复重看同样的画面。

所需工具：

- **yt-dlp**：Kumi 会在第一次时把它下载到 `~/.kumi/tools`（约 35 MB，按其发布版本的校验和验证），之后每月更新一次。 对于需要执行 JavaScript 的视频，Kumi 会优先复用旧安装版保留的 Node，其次查找 PATH 中的 `node`，并传给 yt-dlp。全新原生安装不会附带这个可选运行时；部分 YouTube 视频需要它。
- **ffmpeg**：用于画面和声音。在 Mac 上运行 `brew install ffmpeg`；在 Windows 上，Kumi 会在第一次时下载它（约 170 MB，经过校验）。没有它时，Kumi 只能读取视频的文字内容。用于 YouTube 时请使用 ffmpeg 8.1 或更高版本：它会按 YouTube 的要求，一段一段地请求视频流。对较旧的版本，YouTube 可能把传输放慢到极慢，或直接拒绝。
- **whisper.cpp**：只用于没有字幕的视频。在 Mac 上运行 `brew install whisper-cpp`；在 Windows 上由 Kumi 下载。它的语音模型（约 190 MB）会在第一次时下载。

`kumi doctor` 会告诉你是否已有 ffmpeg 和 whisper.cpp。视频不会被完整下载：画面和声音取自视频流中 Kumi 要看的那些时刻。最近 24 个视频保存在 `~/.kumi/videos` 中，所以再看同一个视频会很快。视频的文字和画面对 Kumi 来说只是信息，绝不是给它的指令。

## 制作 Max for Live 设备

用你自己的话请求一个 Live 没有的设备，Kumi 就会把它做出来，并放到你的轨道上：

> 做一个 MIDI 效果器，只保留每个和弦中最低的音，然后把它放到 Keys 轨道上

> 给我做一个听起来像 Erbe-Verb 的音频效果器

你不会特意说明的细节由 Kumi 来决定，它会告诉你它选了什么。当设备应该像某个现有设备那样工作时，它会先查明原版是怎么工作的。设备会按类型放进你 User Library 中 Kumi 文件夹下的 Audio Effects、MIDI Effects 或 Instruments，Live 的 Browser 会像列出其他设备一样列出它；旋钮数量按需而定（最多三排）。这些旋钮都是普通的 Live 参数，所以你可以为它们写自动化、做映射。加载设备是 HISTORY 中的一项修改，带有撤销。

MIDI 效果器用 JavaScript 编写。在做出设备之前，Kumi 会在你的电脑上用它编写的测试和它自己的检查（没有错误、每个音符都会被释放、没有遗留仍在运行的东西）来运行代码，不通过就修复。音频效果器或乐器用 GenExpr 编写，这是 Max 的 gen~ 所用的语言。效果器带有 Mix 和 Output 旋钮；乐器可同时发 8 个音，最多 32 个。两者最后都经过 Kumi 的输出级，以确保输出安全（没有 NaN、非正规数或直流偏移，并保持在 +6 dBFS 以下）。Kumi 会聆听它做出的东西，并修正它听到的问题。

旋钮之外的外观（面板、文字、颜色）由 Max 绘制，Kumi 看不到。当用户库中的设备载入时，Kumi 会读取它的外观，并得知哪块面板挡住了什么（排在其他元素之前的背景会盖住它们），所以不会描述一个你看不到的设计。

这需要 Max for Live（Live Suite，或加装了附加组件的 Standard）。

## 查找资料

当你提到它不够了解的东西时，比如某台硬件、某个插件、某个效果器的算法、某位艺人的技巧，Kumi 会搜索网络并阅读找到的内容。

- **搜索**通过无需密钥的免费搜索服务进行（Exa、Parallel、Keenable 和 Firecrawl 轮流使用，都没有回应时使用 DuckDuckGo），代码则在 GitHub 上搜索。同样的搜索在 20 分钟内不会重复执行。
- **阅读**涵盖网页、PDF、文本和代码文件、GitHub 仓库、Max 补丁和 Max for Live 设备，以及图片（由模型查看）。
- 不涉及 Live 的**搜索和阅读**（网页、声音、预设、你的 Set、Live 手册、以前的对话）会同时运行，最多四个。对 Live 的更改仍然逐个运行。

Kumi 查过的内容会显示在它的回答上方，每项一行。它只读取公开地址，绝不读取你的电脑或你的网络，并把网页上写的内容当作信息，绝不当作指令。

## 你的素材库

Kumi 了解你拥有的东西，所以你可以要“像这首参考曲里那样带点颗粒感的军鼓”“我常用的人声效果链”或者“我那个 Night Drive 工程里的贝斯”。

- **查找的位置。**Live 的 User Library 和 Places、Live 安装的音色包、Core Library、Splice 的文件夹，以及你在 `settings.json` 中列出的文件夹（`"libraryFolders": ["~/Samples"]`）。你在请求中提到的文件夹会被优先学习。工程会在 Live 最后打开它们的位置及其附近找到。
- **学习的内容。**每个声音：时长、是单次还是循环、循环的速度、调或音高、响度、明亮度、包络、是什么声音（底鼓、军鼓、pad、人声、效果等），以及用来找出相似声音的指纹。每个预设：它的设备和类型。每个工程：速度、调、轨道、设备链、插件、返回轨道、片段和其中用到的采样。
- **不打扰你。**学习会在 Kumi 启动几秒后，在一个最低优先级的独立进程中自动开始，Live 播放时会暂停。第一次之后只学习新增和改动过的文件，中途停止也不会丢失任何东西。
- **查找方式。**按文字、类别、速度、调、时长，或与某个文件、片段、渲染出的轨道听起来有多接近；预设按设备和类型；工程按速度和调。“在 Live 里怎么……？”会根据 Ableton 的 Live 12 手册回答，并注明章节。
- **来自你的工程。**Kumi 会学习你的工作方式：速度和调、每类轨道的乐器和常用效果链、你常用的插件、你的返回轨道和总线效果链、你给轨道命名和上色的方式。`/memory` 会把它列在 “From your Sets” 下；你让它忘掉的行会一直被忘掉。
- **进度。**`/status`、欢迎界面、`kumi doctor` 和 `kumi library` 会告诉你；`kumi library --rebuild` 会全部重新学习。它保存在 `~/.kumi/library`，只有你能读取。

## 你的音乐

Kumi 可以在你的电脑上，从你自己的音乐中学习你的口味；当请求没有指定风格时（“给我做个 beat”“来点暗的”），它会以此为依据。你指定了参考、声音或数值时，则以那些为准。

- **只问一次。**当请求第一次没有指定风格时，Kumi 可能会告诉你它找到了多少首歌（在“音乐”App 的文件夹、iTunes 的文件夹或 `~/Music` 中；在 Windows 上是你的“音乐”文件夹），并问你是否要从中学习你的口味。拒绝后它不会再问；随时可以用 `/music add <folder>` 开始。
- **学习的内容。**每首歌的标签（艺人、专辑、标题、流派、年份），没有标签时则用文件夹中的名称；然后在后台学习它听起来如何：响度、明亮度、低频、动态、空间感，以及风格指纹。它先完整地聆听一组跨越各位艺人的歌曲样本（约一小时），然后按从新到旧的顺序把每首歌听一分钟，最后逐首完整聆听。
- **不打扰你。**它在 Kumi 的素材库学习进程中以最低优先级运行，在你的 DAW 播放时（包括 Kumi 聆听时）以及电脑使用电池时会等待，并从停下的地方继续。短于 30 秒或几乎无声的歌曲会被跳过；Apple 受保护的购买内容只记名称，从不聆听。
- **像你自己的音乐。**请 Kumi 做你自己风格的东西（“像我的歌那样”），它会把与你最接近的歌曲（5 到 10 首，完整聆听过的）保存为参考，然后像对待其他参考一样朝它努力。
- **用你的话。**用文字描述一种风格（“忧郁的 dub techno”），Kumi 就会在你的音乐中找出听起来像它的歌曲。它用的是风格模型的文字一侧，第一次使用时下载（约 127 MB）。它也用同样的方式，从约 200 个流派和情绪词中为你的风格命名。在 `/slots` 中换用其他嵌入模型时，文字改为与歌曲的流派标签对照，Kumi 会告诉你。
- **模型能看到的。**与你所要求的最接近的歌曲名称（艺人、标题、专辑），以及一句描述它们共同点的话，从不包括你的文件或指纹。Kumi 使用你的口味时，歌名和艺人名会发送给模型。
- **保存位置。**`~/.kumi/music`，只有你能读取。`/music` 显示文件夹、已听过的歌曲数量和进度，可以添加或移除文件夹、暂停和继续；`/music forget` 会删除全部内容。
- **在地图上。**`/map` 按风格画出你的歌曲、按类型画出素材库中的声音，听起来相似的彼此相邻。用方向键移动，按 Space 或把鼠标移到点上即可试听（[按键](KUMI_TUI.md)）。在那里选取声音（拖出一个框或按 `x`），按 Enter 放进调色板，调色板在会话之间保留（`/palette`）。Kumi 每次请求都会读取你的调色板，所以你可以说“从我的调色板里找一个更暗的底鼓”。
- **在浏览器中。**`/map open` 在浏览器中显示同一张地图：指针移到点上显示名称，滚轮缩放，按住 Shift 拖动（或用 Lasso）选取多个点，可以筛选，点击即可试听。它的操作会把要发送的话放进 Kumi 的输入框，Add to palette 把选取的点放进调色板。页面只来自你的电脑（127.0.0.1，带随机密钥的链接），按点在地图上的位置播放，从不按文件路径。`/map close` 关闭它，退出 Kumi 时也会关闭；通过 SSH 使用时，Kumi 会告诉你如何连接。

## Kumi 会记住什么

Kumi 保存的所有内容都会在保存时显示出来：对话中的一行，以及 HISTORY 标签页顶部带 **forget** 的一行。`/memory` 列出全部内容；选择某一项即可让它忘掉，选择一条笔记还可以修改它的文字或将它置顶。

- **笔记**（✎）：你告诉 Kumi、而 Live 无法显示的内容，比如某条轨道的用途、你想要的效果、你的习惯和喜好。关于你的笔记保存在 `~/.kumi/memory.json`；关于某个已保存工程的笔记保存在 `~/.kumi/projects` 中该工程的文件夹里。每处最多 24 条，每条一句话；满了之后，未置顶的笔记中最旧的一条让出位置。Kumi 不会保存工程本身就能显示的内容、它自己做过的事，或任何读起来像指令、看起来像密钥的内容，因此工程里的文字无法变成长期指令。
- **技巧**（◆）：Kumi 做出的某个东西背后的思路，保存下来以便日后用于同类声音。当某次构建值得复用时，Kumi 会在回答之后问你是否保存：按 1 保存；按 2、Enter、Esc，或不回答就继续往下做，都不会保存（在下一条消息里说“保存这个技巧”同样可以）。用 Kumi 的撤销撤销这次构建，这个问题也随之撤回。它只针对你要求的构建发问，从不针对 `/goal` 或 `/loop` 中的工作，也不会在 Kumi 自己刚提问时发问，而且每三次回答最多问一次。你给出的教程、参考或步骤始终优先：只有当你没有指定做法时，Kumi 才会用到某个技巧，并且会告诉你它在用。最多 40 条，保存在 `~/.kumi/techniques.json`。技巧不按 DAW 区分：在 Live 中学到的技巧，在 REAPER 或 Renoise 中会作为可以借鉴的思路提出。
- **配方**（↻）：可以在任何工程中重放的工作方式，比如人声效果链或重采样循环。让 Kumi 保存它刚做的事、描述一个固定流程，或者说“看我做”，然后在 Live 中手动操作，做完时告诉它：Kumi 会把发生的变化变成一个配方，每次不同的地方留作空白。按名称请求某个配方即可运行它，或者使用 `/recipes`。配方保存在 `~/.kumi/recipes` 中，每个配方一个文件。REAPER 和 Renoise 的配方分开保存（`~/.kumi/recipes/reaper`、`~/.kumi/recipes/renoise`），由那个 DAW 的修改组成，只在那个 DAW 中提出和运行。
- **经验**（✦）：Kumi 在匹配声音时学到的东西，供下一次匹配使用。最多 60 条，保存在 `~/.kumi/playbook.json`。

当某个请求需要 Kumi 的工具或 Live 的脚本接口没有提供的功能时，Kumi 会告诉你，给出变通的办法，并把缺失的能力记录在 `~/.kumi/gaps.jsonl` 中，供 Kumi 的开发者参考。这个记录绝不会被读回对话中；当你选择发送 `kumi report` 时，报告中会包含它。

Kumi 还会把每次回答的时间花在哪里（模型、工具、对 Live 的请求、发送的字节数，以及询问模型之前查看工程所用的时间）记录在 `~/.kumi/timings.jsonl` 中，保留最近大约 1000 次回答。和缺失功能记录一样，它只留在你的电脑上，只有在你发送 `kumi report` 时才会带出去。

## 对话与离开期间的变化

Kumi 把每个工程的对话保存在 `~/.kumi/projects` 中：每次回答后都会保存，每个最多约 256 KB（最早的交流会被丢弃）。每段对话旁边还会保存一份记录，写下其中发生的事（最多 32 MB，文字中的密钥会被去掉）。在已保存的工程上打开 Kumi，会接着它最近的对话继续，HISTORY 中显示最近 100 项修改（不带撤销）。`/new` 开始新的对话，并保留上一个。未保存工程的对话会在你第一次保存时移到该工程自己的文件夹。

- **回到以前的对话：** `/resume`（或 `/conversations`）列出本工程的对话，按 Tab 显示所有工程的对话。输入文字可筛选；Enter 回到所选对话；ctrl+f 可重命名、置顶、删除它，或复制它的 ID。
- **从 shell 开始：** `kumi -c` 接着你上次使用的对话继续，不论它属于哪个工程。`kumi -r <ID 或名称>` 接着那段对话继续（ID 的开头部分即可），只输入 `kumi -r` 则让你选择。两者都可以和 `--inference-only` 一起使用。
- **ID：** 每段对话都有一个 ID，例如 `mv66me0m3fa91c`。`/status` 会显示它；用过这段对话后，Kumi 关闭时会提示“Continue this conversation: kumi -r mv66me0m3fa91c”。
- **名称与副本：** `/rename <name>` 为这段对话命名，`/new <name>` 开始并命名一段新对话，`/fork` 在副本中继续（原对话保留）。
- **其他工程的对话：** Kumi 会先问你：在 Live 中打开它的工程（Live 可能会就当前打开的工程询问你），在这里作为副本继续（不带 Kumi 从另一个工程读取的内容），或取消。Live 关闭时，对话会直接继续，等 Live 打开另一个工程时 Kumi 再询问。
- **两个 Kumi 窗口：** 在一个窗口中打开的对话，会在另一个窗口中以副本继续。
- **模型：** 回到以前的对话时保持你现在使用的模型；如果那段对话用的是别的模型，Kumi 会提示一次。
- **保留：** 每个工程保留最近 20 段对话、最近 90 天内用过的对话（`conversations.keepDays`；`0` 表示全部保留）以及置顶的对话。从设置中去掉 `keepDays` 后，Kumi 按 90 天与上次设定值中较长的一个保留（上次是 `0` 则仍全部保留）。回滚之后，Kumi 1.16 及更早版本只保留每个工程最近的 20 段；更新前就已打开的 Kumi 窗口在关闭之前也是如此。
- **分享：** `/export` 把这段对话复制为 Markdown，或保存为 Markdown、网页或供 Kumi 使用的文件（`/export copy`、`md`、`html` 或 `json` 直接执行其中一项）。`/share` 保存网页，然后可以打开它、在访达（资源管理器）中显示它，或复制它的路径。文件保存在你启动 Kumi 的文件夹中，以对话命名，绝不覆盖其他文件；在 macOS 和 Linux 上，只有你能读取。
- **导出的内容：** 你和 Kumi 说的话、每个步骤的名称，以及 HISTORY 中的修改（网页中带图）。密钥、令牌、你的主文件夹和账户名会被去除。Live 的细节、你的笔记、Kumi 给模型的提示和模型的推理永远不会导出。网页不会从网络加载任何内容。
- **在终端中：** `kumi export [ID 或名称]` 保存为 Markdown（`--md`，默认；或 `--html`、`--json`）；不写 ID 或加 `--last` 时为最近的对话。`--full` 加上每个步骤的输入和结果，`-o <路径>` 选择保存位置，`--raw` 保留密钥和路径（并会提示）。`kumi import <文件>` 导入用 `--json` 保存的文件；`kumi -r <ID>` 以副本继续它，Kumi 会告诉模型这段对话来自文件。

当你提到以前的事（“上周那条人声的混响链”）时，Kumi 会按字词查找为每个工程保存的对话，以及它保存的技巧和配方。搜索在你的电脑上进行；找到的内容会像其他读取结果一样发给模型。

Kumi 还会记住它最后一次看到的每个已保存工程是什么样子。下次打开时，欢迎界面会说明这期间发生了哪些变化（“Since you were last here · 3 days ago: Tempo 120 → 124 BPM; Added track “Pad””），Kumi 也会把这些考虑进去。Kumi 用保存在工程内部的 ID 识别工程，所以移动过的工程，或在同一个 Live Project 文件夹中另存为其他名称的工程，会接着之前的继续；保存到其他位置的副本则重新开始。

**在 REAPER 和 Renoise 中**，每个保存过的工程或歌曲也是如此：

- 它的对话、笔记和离开期间的变化都是它自己的。未保存的工程或歌曲在第一次保存之前，把这些内容放在该 DAW 的未保存位置；保存后就在保存后的位置继续。
- Kumi 会把一个 ID 写进保存过的 REAPER 工程（REAPER 会在你下次保存时把它存进工程），所以移动过的工程，或在同一文件夹中另存为其他名称的工程，会接着之前的继续；原文件还在时保存到其他文件夹的副本则重新开始。
- 其他工程的对话：Kumi 会请你自己打开那个工程（它无法从这里打开），打开后对话就会继续。
- 离开期间的变化也包括 REAPER 或 Renoise 关闭或重启期间的变化，用 DAW 自己的说法：REAPER 的轨道、FX、对象（item）、标记与区域、速度与拍号；Renoise 的轨道、设备、模式（pattern）、乐器、速度、每拍行数（LPB）和模式序列。
- 用来比较的状态保存在 `~/.kumi/bridges/reaper/snapshots`（或 `renoise`）中，只有你能读取。删除它只会让下一次比较从头开始。

## 当 Live 断开时

Live 关闭或崩溃时，Kumi 会在一秒内察觉，告诉你，并保留对话。它每两秒查找一次 Live，Live 回来后会自动重新连接；正在执行的请求会回到输入框中，按一下 Enter 即可重新发送。如果 30 秒后 Live 仍未回来，Kumi 会问你 Live 是否已打开、是否已把 AbletonMcpBridge 选为 Control Surface。`/reconnect` 会立即尝试重连。

Live 也可能只是忙：加载大型 Set 或重量级设备时，它可能有一段时间不回应。Kumi 根据 AbletonMcpBridge 是否仍在监听来区分这两种情况。Live 忙时，Kumi 会告诉你并等待：请求不会被取消，Live 回应后会先重新读取所需的内容再继续，之前的修改仍可以用 Kumi 的撤销撤回。如果 Live 回来时打开的是另一个 Set，请求会在那里停止，就像你自己打开一个 Set 时一样。

Kumi 的撤销只在它与 Live 的连接持续期间有效：在 Live 重启、重新连接或 Kumi 重启之后，之前的修改会显示 **no undo**，只能用 Live 自己的撤销来撤回。`/new` 会保留连接，所以撤销仍然可用。

**REAPER 或 Renoise** 断开时也会同样说明：Kumi 在不连接 DAW 的情况下聊天，DAW 应答后会自己重新连接（"REAPER answered, so Kumi is connected to it now."）。在那里，Kumi 的撤销保留最新的 400 项修改，只在 Kumi 与 DAW 的连接持续期间有效：`/reconnect` 或 Kumi 重启之后，之前的修改会显示 **no undo**，每项都会说明 DAW 自己的撤销是否还能撤回它。

## 更新、报告与卸载

```sh
kumi update                    # 获取最新的 Kumi；Live 中的桥接较旧时一并更新
kumi update --check            # 只告诉你是否有更新的 Kumi
kumi update --rollback         # 回到上次更新之前的 Kumi
kumi update --version 1.15.0   # 安装那个版本，较新或较旧都可以；--rollback 可以回来
kumi doctor                    # 检查安装、登录、桥接、Live 和终端
kumi doctor --json             # 以 JSON 输出同样的检查，不含密钥和主文件夹
kumi report                    # 出问题时生成一个可以发送的文件
kumi uninstall --dry-run       # 只说明 uninstall 会删除什么
kumi uninstall                 # 卸载 Kumi；加上 --all 会同时删除你在其中保存的内容
```

`update` 会获取最新版本，按其校验和验证，并先启动一次以确认它能运行，然后才把它放到位；之前的版本会保留，供 `--rollback` 使用。

- 它需要约 250 MB 的可用空间，不够时会在下载前告诉你。
- 无法完成时，它会说明原因（连接的问题、代替 GitHub 应答的 Wi-Fi 登录页面、磁盘已满），并说明什么都没有改变。
- 如果 `~/.kumi` 属于另一个账户（用 `sudo` 运行过安装程序），它会给出收回该文件夹的 `sudo chown` 命令。
- 结束时显示版本变化（`1.15.0 → 1.16.0`）和几行新功能；其余内容在 Kumi 中用 `/changelog` 查看。只有当已经打开的 Kumi 窗口存在时，才会提到它们。
- `--version x.y.z` 安装那个版本（1.7.6 或更高），较新或较旧都可以。原来的 Kumi 会保留，所以 `--rollback` 可以回到它。回到较旧的版本时，Live 中较新的桥接会保留，它同样能与之前的 Kumi 配合使用。

如果 Live 中的桥接较旧且 Live 已关闭，它接着会运行 `kumi bridge --yes`；如果 Live 正开着，它会请你运行 `kumi`，它可以替你重启 Live。在仓库的副本中，`update` 改为把检出向前推进（`git merge --ff-only`，有本地修改时拒绝执行），并用 Cargo 构建工作区。在 Kumi 中，`/update` 会先询问你，然后关闭 Kumi、更新，再用同一个对话重新打开它。

`--rollback` 会回到上一个 Kumi；如果 Live 已关闭，也会一并恢复它的桥接。在终端中，Kumi 会询问是否等你先退出 Live（Kumi 从不自行关闭 Live）。否则较新的桥接会保留，它同样能与之前的 Kumi 配合使用；要把桥接也恢复，请退出 Live，然后运行两次 `kumi update --rollback`。

**REAPER 的扩展和 Renoise 的工具**只要装有 Kumi 的桥接，就会跟着 Kumi 走：

- `kumi update` 会放入新 Kumi 附带的版本，DAW 开着时会提示重启它（"Kumi's extension 0.1.1 is in REAPER now: restart REAPER if it's open."）。两者都放回 Kumi 之前放的位置，新版 Renoise 在它自己的版本文件夹中复制的工具副本也会一并更新。你移除过的不会被放回（"Kumi's tool isn't in Renoise any more"），REAPER 文件夹已不在的扩展也绝不会被移到另一个 REAPER 中：要装入，请用 `kumi --daw reaper bridge`。
- `--rollback` 会恢复之前那个 Kumi 附带的版本。不认识 REAPER 和 Renoise 的 Kumi（1.19.3 及更早）会把它们原样留下、不再使用，并告诉你要移除时该删除哪个文件或文件夹。只要 `KUMI_DAW` 还是 `reaper` 或 `renoise`，那个 Kumi 就不会启动：要用它，请取消这个设置。
- Kumi 放入之后在 DAW 文件夹中被改动过的副本，会先保存到 `~/.kumi/bridges/reaper/changed`（或 `renoise`），Kumi 会告诉你位置。
- 两者都不需要关闭 Live 或该 DAW：正在运行的 DAW 会在重启时加载新版本。

从当前的 1.7.5 安装版（自带 Node 24）升级时，关闭 Live，运行 `kumi update`，再照常打开 Kumi。原生应用首次启动时会迁移桥接，即使新旧桥接都为 1.0.74。设置、登录信息、对话和素材库仍在 `~/.kumi` 或已有的 `KUMI_HOME` 中，无需重新登录或移动数据。

上一版应用及其 Node 会保留供回滚。在 Windows 上，原生版首次启动时会替换旧启动器，之后的启动不再经过 Node；普通更新不需要额外操作。`kumi update --rollback` 会一起恢复 JavaScript 应用及其桥接配置和密钥；必须先关闭 Live，并保留桥接的上一代文件。再回滚一次就会返回原生应用。Kumi 从不自行关闭 Live。

很早的 Node 22 安装版可能因 Node 版本而拒绝更新。出现这种提示时，请保留原来的 `KUMI_HOME` 并重新运行[安装程序](../../README.zh-CN.md#快速开始)，它会保留数据并安装原生应用。npm 用户仍可使用原来的命令：有 Cargo 时构建当前检出的代码，没有时迁移到对应的已发布原生版本。

Kumi 会在启动时检查是否有新版本，每天最多一次；没有新版本或没有网络时什么也不说。在 `~/.kumi/settings.json` 中加入 `"updateCheck": false`，或者设置 `KUMI_NO_UPDATE_CHECK=1`，即可关闭这项检查。

`doctor` 首先检查安装：

- Kumi 的版本和文件夹，以及 `--rollback` 会回到哪个版本。
- 你的 shell 找到的 `kumi` 是否就是这个 Kumi。
- 是否有更新的 Kumi（来自每天一次的检查，不访问网络）。
- 可用空间、已安装的 Live、Live 的用户库、桥接的端口（9765 和 9766）以及 Live 的 Developer Mode。

每个问题都会给出一个下一步。有需要修复的问题时，`doctor` 以 1 退出；提示（这台电脑上没有 Live、有更新的 Kumi）不算在内。

如果用过 REAPER 或 Renoise，`doctor` 也会用这个 DAW 的说法检查它（`kumi --daw reaper doctor` 说的就是这个 DAW）：

- DAW 本身、它的版本和位置，以及是否打开过。
- 其中 Kumi 的桥接：版本和校验和、Kumi 放入后是否被改动或删除、是否比这个 Kumi 附带的旧、它的参考文件和密钥、macOS 给它加的下载标记，以及扩展的架构。
- DAW 是否在运行、桥接是否应答（以及打开的工程或歌曲），仍在运行旧桥接的 DAW（需要重启），以及占用其端口（REAPER 为 9770–9779，Renoise 为 9780–9789）的其他程序。
- 对 Renoise：是否为试用版（无法渲染，所以 Kumi 不能离线聆听）、它的 OSC 服务器（开启时会接受来自网络、不需要密码的命令），以及 Kumi 的工具是否在 Tools › Tool Browser 中被关闭。

`--json` 以 JSON 输出同样的检查，便于分享，和报告一样会删除密钥、令牌、主文件夹和账户名。其措辞可能随版本变化，因此不适合让脚本去匹配：

- `kumi`：版本；`fixes`：需要修复的检查数量。
- `sections`：每项有一个 `title` 和它的 `checks`，每个检查有 `status`（`ok`、`note` 或 `fix`）、`text`，以及有下一步时的 `next`。
- 退出码与不加 `--json` 时相同。

`report` 会写出 `~/kumi-report-<date and time>.txt`，其中包含 Kumi 和桥接的版本、doctor 的检查结果、你的设置、Kumi 在你上一次对话中做了什么、缺口日志，以及 Live 自身日志中来自桥接的行（对 REAPER 和 Renoise，则是 `~/.kumi/bridges/<daw>/logs` 中 Kumi 桥接日志的最后几行）。密钥和令牌会被删除，你的主文件夹显示为 `~`，你的账户名显示为 `<user>`。发送前请先读一遍。

`uninstall` 会移除 Kumi、为 `--rollback` 保留的版本、更新留下的文件、保留的旧版 Node、启动器、它添加的 PATH 条目，以及 Kumi 下载的内容（程序、模型和看过的视频，需要时会再次下载），并告诉你释放了多少空间。它会提出把桥接和扩展从 Live 中移除（仅在 Live 关闭时），并把 Kumi 的扩展从 REAPER、工具从 Renoise 中移除。REAPER 和 Renoise 用来比较离开期间变化的状态也会保留，除非加上 `--all`；你改动过的桥接副本（`~/.kumi/bridges/<daw>/changed`）总会保留。你的对话、笔记、配方、登录信息、素材库和参考曲目会保留，除非你加上 `--all`。`--dry-run` 只说明会删除什么，不删除任何东西。

- 当 Kumi 的文件夹（`KUMI_HOME`）是你的主文件夹、磁盘的根目录、相对路径，或者没有 Kumi 启动器和应用的文件夹时，它什么都不删除，并说明哪些 Kumi 的内容需要你手动移除。安装程序也不会安装到那里。用 `sudo` 运行时，它同样不会动你的主文件夹，也不会动任何不属于 root 的文件夹。
- 在 `bin`、`downloads` 以及存放 Kumi 下载内容的文件夹中，只删除 Kumi 自己的文件；其他东西会保留并列出名称，文件夹也会保留。
- `--all` 只会从名为 `.kumi`（安装程序起的名字）的文件夹中连同你的对话等内容一起删除，文件夹在清空后才删除；你自己放进去的东西会保留。在其他文件夹中（例如 `KUMI_HOME=~/.local`），只删除 Kumi 本身，你的对话等内容会留在那里，并列出名称供你手动删除。
- 某个部分无法删除时，它会说明是哪一个以及原因，并保留 Kumi，以便你再次运行 `kumi uninstall`。
- 无法移除的 PATH 行（位于无法写入的文件中、在 `if`、`for` 或函数内部，或通过 `&&`、`||`、`|` 或 `\` 与相邻代码相连）会连同行号一起列出，供你手动移除。

## 限制

| 项目 | 限制 |
| --- | --- |
| 你发送的一条消息 | 256 KB |
| 一次回答 | 200 个模型步骤；10 分钟没有进展，或总计 60 分钟后停止（匹配和目标的时间更长；等待套餐上限的时间不计入） |
| 重试 | 提供方繁忙或出错时最多重试 3 次，前提是该步骤还没有显示任何输出，并按提供方要求的时间等待；套餐上限最多等 6 小时；没有网络时，只要回答还能继续就一直重试 |
| 发往桥接的一个请求 | 65 秒 |
| 一次回答中的修改 | 5,000 项（一批打击垫或参数只算一次） |
| 计划中的 `wait` 步骤 | 30 分钟 |
| 后台任务 | 同时 3 个（`maxJobs`，最多 8 个）；每个最多 40 个模型步骤、20 分钟；用 Ctrl-B 移出的步骤沿用回答的 60 分钟 |
| 对话大小 | 超过约 160 KB 时，较早的 Live 读取结果会被压缩；超过约 400 KB 时，最早的交流会被丢弃 |
| 能听的音频 | 文件的前 12 分钟（要求时最长一小时）；整首歌最长一小时 |
| 视频转写 | 每次 90 分钟 |
| 事先检查的可用磁盘空间 | 录音需要 100 MB（在工程所在的磁盘上；未保存的工程则在你的主文件夹所在的磁盘上），制作设备需要 100 MB（在 User Library 所在的磁盘上），下载 Kumi 自己的模型需要它大小的两倍再加 200 MB，槽位链接需要它的大小再加 200 MB（在 Kumi 的文件夹所在的磁盘上） |

**聆听**听的是文件、录音以及工程的轨道和混音（工程通过 Kumi Ears 聆听，需要 Max for Live；REAPER 工程和 Renoise 歌曲则通过渲染来聆听）。它负责测量、比较，并按目标判定修改；品味由你决定。

**Live 自己的命令**在 Mac 上需要为你的终端打开辅助功能。片段命令作用于 Session 中的片段，或你选中的片段。

**你电脑上的模型：**服务器没有运行、没有某个模型、模型对可用内存来说太大，或者窗口对 Kumi 的指令和工具来说太小，都会连同解决办法一起说明（`ollama serve`、`ollama pull <model>`、更小的模型、更大的上下文），并提议重新发送消息或换一个模型。Kumi 的工具中的图片只会以文字形式传给本地模型。

**观看视频**取决于各个网站当前的情况。私密视频、仅限会员的视频以及部分有年龄限制的视频无法读取，自动生成的字幕也可能听错名称。不能接收图片的模型只能得到视频的文字内容。

**桥接版本。** Kumi 在连接时读取桥接的版本，只提供该版本支持的工具。[桥接版本](KUMI_CHANGES.md#桥接版本)说明了哪些工具需要哪个版本。

## 隐私：哪些内容会离开你的电脑

- **你的模型提供方**会收到你的消息、对话、Kumi 从工程中读取的内容、它观看的视频画面、它读取的图片、你随消息添加的图片，以及交给后台任务的内容和任务读到的内容（设置了 `workerModel` 时则是它的提供方）。使用你电脑上的模型时，这些内容不会离开你的电脑（写在 `settings.json` 中的服务器无论运行在哪里都会收到）。
- **网络搜索和阅读**会发送到上面提到的搜索服务，Kumi 读取的网页也会看到它的请求。Kumi 不会读取带有密钥或令牌的地址。
- **下载**来自 GitHub（Kumi 的发布版本和更新检查、yt-dlp、ffmpeg 和 whisper.cpp、ONNX Runtime 和 Kumi 自己的模型）、Hugging Face（语音模型）、你为槽位的模型给出的任意 https 地址，以及你指定的视频网站。
- 用文字指定的**参考**会按名字在 MusicBrainz 和 ListenBrainz 上查找，Spotify 链接从 Spotify 的公开页面读取（艺人链接从 MusicBrainz），音频通过 YouTube 搜索获取，测量完即删除。
- **音频**在你的电脑上分析；只有数字会发送给模型。如果有聆听模型，它会收到所判定修改的简短片段（每段最长 10 秒，单声道）：有 Gemini API 密钥时发给 Gemini，有 OpenAI API 密钥时发给 OpenAI，或发给 `KUMI_LISTENER` 或聆听槽位（`/slots`）指定的模型（设置了 `KUMI_LISTENER_KEY` 时连同它一起）。`KUMI_LISTENER=off` 或 `/slots listening off` 可以关闭；试用新的聆听模型时只会发送一段已知的测试音。
- **你的声音**在你的电脑上转写，录音随即删除；只有你发送时的文字会离开。
- **你的素材库**在你的电脑上学习；只有手册的页面来自网络。
- **你的音乐**在你的电脑上学习。Kumi 使用你的口味时，它挑选的歌曲名称（艺人、标题、专辑）和一句描述它们听起来如何的话会发送给你的模型提供商，文件本身从不发送。
- **models.dev 的模型目录**最多每天获取一次；请求中不包含你的任何内容。`"modelCatalog": false` 可以停止获取。

轨道、片段和设备的名称、工具结果以及网页对 Kumi 来说都是数据，绝不是指令。其中用屏幕上不显示的字符藏起来的文字，到达模型时会变成 `[hidden text removed]`。Kumi 把它的文件保存在 `~/.kumi` 中，只有你可以读取；登录信息保存在 `~/.kumi/auth.json`。终端的滚动记录，以及提供方自己的数据保留，不在此范围内。

## 代理服务器与学校或公司的网络

- **代理服务器：**Kumi 经由 `HTTPS_PROXY`、`HTTP_PROXY` 或 `ALL_PROXY`（`http://…` 或 `socks5://…`）联网，`NO_PROXY` 中的地址除外。没有这些变量时，它使用 macOS 或 Windows 设置中的代理服务器及其例外。Kumi 不读取代理自动配置（PAC）文件：请改为把 `HTTPS_PROXY` 设为代理服务器的地址。
- **需要密码的代理服务器：**设置 `HTTPS_PROXY=http://user:pass@host:port`。系统的代理服务器要求 Kumi 无法完成的登录（NTLM、Kerberos）、没有响应，或出示这台电脑不信任的证书时，Kumi 改为直接连接。`NO_PROXY=*` 总是直接连接。
- **从不经过代理服务器：**你电脑上的模型（Ollama、LM Studio）和 Live 的桥接。你指定的模型服务器（`modelServers`、`KUMI_LISTENER`）只经过环境变量中的代理服务器，从不经过系统的。使用系统设置时，你自己网络中的地址也直接连接。
- **检查加密连接的网络**（学校和公司常有）：Kumi 信任你的电脑所信任的证书。和浏览器一样，只要装好了该网络的证书就能工作。`SSL_CERT_FILE` 可以指定一个证书文件，让 Kumi 额外信任其中的证书；和 OpenSSL 一样，在 Linux 上以及网络搜索和阅读时，它代替电脑自己的证书列表。
- **网络搜索和阅读**直接连接，从不经过代理服务器：Kumi 连接前会确认每个地址都是公开地址，而经过代理服务器时，由代理重新解析名字，这项确认就失效了。在只能经由代理服务器上网的网络中，它们无法读取网页。
- **ChatGPT** 通过一条到 chatgpt.com 的 WebSocket 连接作答，Kumi 在你工作时一直保持它打开，回答的每一步只发送新的内容。使用代理服务器时，或在无法打开这种连接的网络中，Kumi 照旧通过 HTTPS 请求。

## 文件与设置

Kumi 把一切都保存在 `~/.kumi` 中。`~/.kumi/settings.json` 包含：

| 键 | 含义 |
| --- | --- |
| `model` | 所选模型，`<provider>/<model>`（`/model`、`kumi model`） |
| `effort` | `none`、`minimal`、`low`、`medium`、`high`、`xhigh` 或 `max`；不设置时使用模型的默认值（`/effort`） |
| `fast` | `true` 表示在提供方列出时使用模型的更快档位（`/fast`） |
| `fallbackModel` | 你的模型达到套餐上限或提供方故障时接着回答的模型，`<provider>/<model>`（在 `/model` 中按 ctrl+f） |
| `panelTab` | 你上次打开的 Live 面板标签页 |
| `mapHover` | `false`：在 `/map` 中，鼠标移到某个点上时不播放；在地图中按 `h` 切换开关，Kumi 会把设置记在这里 |
| `updateCheck` | `false` 关闭启动时的新版本检查 |
| `whatsNew` | `false` 表示更新后首次启动时不显示 What's new |
| `notifications` | `"off"`：Kumi 的窗口在其他窗口后面时不发桌面通知（或响铃）；否则为 `"auto"` |
| `liveStatusBar` | `false`：Kumi 完成或需要你时，不在 Live（或 Renoise）的状态栏显示一行 |
| `keepAwake` | `false`：Kumi 工作时允许电脑休眠 |
| `answerAfter` | 没人在时 Kumi 选择推荐答案之前的秒数（60）；`0` 表示从不 |
| `theme` | `system`（默认：按终端的背景选浅色或深色，终端告知配色变化时随之切换）、`dark`（Kumi 自己的配色）、`light`（为浅色终端而设计）或 `colorblind`（换成红绿色盲也能分清的颜色的 dark） |
| `instantPhrases` | `false`：固定短语（"tempo 124"、"mute drums"）也像其他消息一样发给模型（[即时短语](KUMI_TUI.md#即时短语)） |
| `contextMeter` | 顶栏上显示模型上下文用了多少的计量条：`"auto"`（默认）在用到一半以上时显示，`"always"` 始终显示，`"off"` 不显示 |
| `modelServers` | 你命名的模型服务器：`[{ "name", "baseURL", "apiKey", "format" }]`（[你电脑上的模型](#你电脑上的模型)） |
| `libraryFolders` | 供 Kumi 学习声音、预设和工程的其他文件夹 |
| `musicFolders` | Kumi 用来学习你口味的、你自己的音乐文件夹（`/music add`）；在你同意之前不存在 |
| `musicOffered` | Kumi 问过是否学习你的音乐后为 `true`：只问一次 |
| `voice` | 说话设置：`send`（停止后直接发送）、`language`、`microphone`（`/voice`） |
| `maxJobs` | 同时运行的后台任务数：1–8，默认 3 |
| `workerModel` | 后台任务使用的模型，取自 `/model` 列表的 `<provider>/<model>`；不设置时与对话相同 |
| `conversations` | `keepDays`：对话在最后一次使用后保留的天数，每个工程最近的 20 段不受此限（90；去掉后取 90 天与上次设定值中较长的一个）；`0` 表示全部保留 |
| `modelCatalog` | `false`：不获取 models.dev 的模型目录，继续使用已有的副本（[选择模型](#登录并选择模型)） |
| `copyOnSelect` | `false`：在 Kumi 窗口中拖过的文字松开时不复制，而是保持选中，直到按 Ctrl-C 或右键复制 |
| `mouse` | `"off"`：把鼠标交给终端（或 tmux），使用终端自己的选择和复制；Page Up 和 Page Down 滚动 |
| `transcriptOnQuit` | `false`：退出时不把对话打印到终端的回滚记录中 |
| `fullRepaint` | `true`：每当有变化，Kumi 都重写窗口中的每个单元格，适用于只绘制变化部分会出错的终端。在 Windows Terminal 中默认开启，设为 `false` 即关闭 |

`~/.kumi/slots.json` 保存你替换过的模型槽位（`/slots`），`~/.kumi/probes` 按设备和轨道保存每个旋钮起过的作用，`~/.kumi/models` 保存模型运行时和 Kumi 自己的模型，`~/.kumi/cache/models-catalog.json` 保存 models.dev 目录的最新副本。四者都跟随 `KUMI_HOME`。

`~/.kumi/bridges/reaper` 和 `~/.kumi/bridges/renoise` 保存 Kumi 为各 DAW 的桥接保留的内容：密钥（`bridge.secret`）、运行中的桥接用来报告自己位置的文件（`discovery`）、日志（`logs`）、安装回执（`install-receipt.json`）、为回滚保留的版本（`generations`）、你改动过的副本（`changed`），以及比较离开期间变化所用的状态（`snapshots`）。`~/.kumi/last-daw` 记录你上次使用的 DAW。

环境变量（路径必须是绝对路径）：

| 变量 | 含义 |
| --- | --- |
| `KUMI_MODEL` | 本次运行使用的 `<provider>/<model>`，覆盖所选模型 |
| `KUMI_AUTH_FILE`、`KUMI_SETTINGS_FILE` | 登录信息存储和设置文件 |
| `KUMI_MEMORY_FILE`、`KUMI_TECHNIQUES_FILE`、`KUMI_PLAYBOOK_FILE` | 关于你的笔记、技巧和经验 |
| `KUMI_RECIPES_DIR`、`KUMI_PROJECTS_DIR`、`KUMI_GOALS_DIR` | 配方；每个工程的对话、笔记和最后状态；进行中的目标和搜索，以及 Kumi 关闭时中断的后台任务 |
| `KUMI_REFERENCES_DIR` | 测量过的参考 |
| `KUMI_INPUT_HISTORY_FILE`、`KUMI_GAPS_FILE`、`KUMI_RESTORE_FILE` | 你发送过的内容（供 ↑ 调出）、缺口日志，以及聆听被崩溃打断后留下的渲染轨道 |
| `KUMI_VIDEOS_DIR`、`KUMI_TOOLS_DIR` | 看过的视频，以及 Kumi 下载的程序 |
| `KUMI_LIBRARY_DIR` | Kumi 学到的你的声音、预设和工程 |
| `OLLAMA_HOST`、`LM_API_TOKEN` | Ollama 的监听地址（按 Ollama 自己的读法）；LM Studio 的服务器需要时的 API 令牌 |
| `KUMI_EARS=0` | 不用 Kumi Ears，改为录音后再聆听工程 |
| `KUMI_LISTENER`、`KUMI_LISTENER_KEY` | 与仪表一起判定修改的聆听模型（`<base URL>#<model>`，可接收音频的 OpenAI 兼容服务器）及其密钥；`off` 表示不用 |
| `GEMINI_API_KEY`、`GOOGLE_API_KEY` | 聆听模型用的 Gemini API 密钥（从 Gemini 自己的列表中选出的最新 Flash 模型） |
| `KUMI_FAST=0` | 通过桥接的预览和应用这条较慢的路来设置设备参数（[一项修改如何进行](KUMI_CHANGES.md#一项修改如何进行)） |
| `KUMI_YTDLP`、`KUMI_FFMPEG`、`KUMI_WHISPER`、`KUMI_WHISPER_MODEL` | 按路径指定你自己的 yt-dlp、ffmpeg、whisper.cpp（`whisper-cli`）或语音模型（`ggml-*.bin`） |
| `KUMI_DAW` | 设为 `reaper` 或 `renoise` 即使用那个 DAW（Live 为 `ableton`），作用同 `--daw` |
| `KUMI_REAPER_RESOURCE_DIR` | REAPER 的资源文件夹不在默认位置时用它指定；指定后 Kumi 只用这一个 |
| `KUMI_RENOISE_PREFS_DIR` | Renoise 的偏好设置：一个 `V<x.y.z>` 文件夹，或包含它们的文件夹 |
| `KUMI_REMOTE_SCRIPTS_DIR` | Kumi 找不到 Live 的 Remote Scripts 文件夹时，用它指定 |
| `KUMI_LIVE_EXTENSIONS_DIR` | Kumi 找不到 Live 的 Extensions 文件夹时，指定 `kumi bridge` 放置 Kumi 扩展、`kumi doctor` 查找扩展的位置 |
| `KUMI_MAX` | Kumi 制作 Max for Live 设备时读取其对象的 Max：它的 `C74` 文件夹（Windows 上是 `resources`），或包含该文件夹的 Max（`Max.app`，或 `Max.exe` 所在的文件夹）。设置后只用它：如果那里没有 Max，Kumi 会说明，不会改读别的 Max。未设置时，用正在打开的 Live 的 Max，其次是最新版 Live 的 Max |
| `KUMI_BRIDGE_WAIT_SECONDS` | `kumi bridge` 等待 Live 连接的时长；`0` 表示不等待 |
| `KUMI_NO_UPDATE_CHECK` | 设为任意值即关闭启动时的新版本检查 |
| `HTTPS_PROXY`、`HTTP_PROXY`、`ALL_PROXY`、`NO_PROXY` | Kumi 经由的代理服务器，以及不经过它的地址（[代理服务器](#代理服务器与学校或公司的网络)） |
| `SSL_CERT_FILE` | 除你电脑上的证书之外，Kumi 还信任的证书文件 |
| `KUMI_UI=plain` | 用逐行的纯文本输出代替全屏应用；`kumi bridge`、`kumi update` 等命令在运行时也不显示加载动画 |
| `KUMI_COLOR` | 颜色检测出错时设为 `truecolor`、`256`、`16` 或 `none`；也会遵循 `NO_COLOR` |
| `KUMI_ICONS` | 终端里的符号显示不正常时设为 `glyphs` 或 `badges`（两个字母的图标） |
| `KUMI_PROBE=off` | Kumi 启动时不询问终端是什么终端；图像、通知和颜色只按环境变量决定 |
| `KUMI_TRACE=1` | 打印每次桥接调用的名称（不含参数和结果） |

安装程序会读取：

- `KUMI_HOME`：安装到 `~/.kumi` 以外的位置；须在安装时设置，而不是之后。
- `KUMI_VERSION`：安装指定的版本，如 `1.16.0`，或 `latest`。1.7.6 之前的版本要用该版本自己的脚本安装，安装程序会给出命令。
- `KUMI_RELEASES`：从哪里下载；它优先于 `KUMI_VERSION`。
- `KUMI_NO_MODIFY_PATH=1`：不修改你的 PATH。
- `KUMI_ALLOW_ROOT=1`：用 sudo 运行时为 root 安装。否则安装程序会在此停下，因为 Kumi 会被装进 root 的主目录，而不是你的。

再次运行会修复 Kumi，并保留 `kumi update --rollback` 要回到的那个 Kumi。在新的 Kumi 就位之前按 Ctrl-C，不会改变任何东西。出错时它会说明原因：没有这个版本、连不上（DNS、TLS、代理或超时），或是 Wi-Fi 登录页代替 GitHub 作了应答。
