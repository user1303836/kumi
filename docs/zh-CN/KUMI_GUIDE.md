# Kumi 指南

[English](../en/KUMI_GUIDE.md) · 简体中文 · [日本語](../ja/KUMI_GUIDE.md)

Kumi 是一个在终端里运行的制作人代理，为你在 Ableton Live 中打开的工程工作。它读取工程，按你的要求进行修改，并把每项修改显示在 HISTORY 中。它能播放、录音和渲染，聆听音频并与参考曲对比，观看视频教程，制作 Max for Live 设备，在网上查找资料，还会记住你的工作方式。

要安装 Kumi 并把它连接到 Live，请按照[快速开始](../../README.zh-CN.md#快速开始)操作。本指南介绍之后的一切。按键与界面请见[命令、按键与界面](KUMI_TUI.md)；修改与撤销如何工作，请见 [Kumi 如何修改你的工程](KUMI_CHANGES.md)。

## 登录并选择模型

在 Kumi 中，`/login` 用于登录（第一次时，Kumi 的设置会先问你）：用你的套餐登录 ChatGPT（浏览器会打开，登录结果通过 `localhost:1455` 返回），或用 API 密钥登录 Anthropic、OpenAI 或 OpenCode，密钥粘贴到只显示圆点的输入框中。Kumi 在保存密钥之前会先向其提供方验证；如果连不上提供方，它会保存密钥，并说明尚未验证。`/logout` 用于退出登录。

| 提供方 | 模型名称 | 登录方式 |
| --- | --- | --- |
| ChatGPT 套餐 | `openai-codex/<model>` | `/login`（浏览器），或 `kumi login openai-codex`（在没有浏览器的机器上加 `--device`） |
| Anthropic API | `anthropic/<model>` | 用 API 密钥 `/login`，或 `ANTHROPIC_API_KEY` |
| OpenAI API | `openai/<model>` | 用 API 密钥 `/login`，或 `OPENAI_API_KEY` |
| OpenCode Zen 和 Go | `opencode/<model>`、`opencode-go/<model>` | 用 API 密钥 `/login`（两者共用一个密钥），或 `OPENCODE_API_KEY` |
| Ollama（在你的电脑上） | `ollama/<model>` | 无需：运行时即可找到 |
| LM Studio（在你的电脑上） | `lmstudio/<model>` | 无需：其服务器运行时即可找到 |
| 其他兼容 OpenAI 的服务器 | `<name>/<model>` | 在 `settings.json` 中写上名称（[见下文](#你电脑上的模型)），需要时附上密钥 |

用 `/login` 保存的密钥优先使用；没有时，Kumi 使用环境变量中的密钥，而且无法从中退出登录（请改为取消设置该变量）。API 密钥只能粘贴到 Kumi 的密钥输入框或 `kumi login` 的提示中，绝不要粘贴到消息里。Kumi 不提供 Claude 和 Gemini 的订阅登录，因为它们的提供方不允许在第三方工具中使用。OpenCode 的 Gemini 模型暂不支持。

`/model` 列出每个提供方的模型，这些模型直接从提供方读取，所以今天发布的模型无需更新 Kumi 就会出现。没有选择模型时，Kumi 使用你已登录的第一个提供方（按上表顺序）的第一个模型，并告诉你用的是哪个。`/effort` 设置模型思考的力度，可选级别取决于该模型；越低回答越快。两者都从你的下一条消息开始生效，并保留到下次使用。更换模型会保留对话，但先前模型的私有推理除外，那部分属于该模型。

当回答因缺少登录或登录被拒而失败时，Kumi 会提出帮你登录，然后重新发送你的消息。当提供方不提供所选模型时，Kumi 会提出让你另选一个。当回答中途断开（比如连接断了）时，Kumi 会从断开处接着回答一次，并在状态行中说明；如果再次断开，Kumi 会提出重新发送你的消息。

### 你电脑上的模型

- **Ollama** 和 **LM Studio** 只要在运行就会被找到，无需登录。`/model` 会把它们各自列为一个提供方（“Ollama · on this computer”），并列出该服务器上的模型；已安装但未运行的会说明如何启动。如果你没有登录任何提供方，Kumi 会从其中能修改工程的模型开始（优先选择已加载的）。
- **其他兼容 OpenAI 的服务器**（llama.cpp 的 `llama-server`、vLLM、Jan 等）写在 `~/.kumi/settings.json` 中。名称就是它的 ID（`llama.cpp` 即 `llama-cpp/<model>`）：

  ```json
  { "modelServers": [
    { "name": "llama.cpp", "baseURL": "http://127.0.0.1:8080/v1" },
    { "name": "Studio PC", "baseURL": "http://192.168.1.20:8000/v1", "apiKey": "…" }
  ] }
  ```

- **给 Kumi 留出空间。** 仅 Kumi 的指令和工具就约有 2.5 万至 3 万个 token。Kumi 会向 Ollama 请求能容纳它们、对话和回答的空间（约 5.7 万个 token，以模型能读取的上限为准），并在 LM Studio 加载的模型空间不足时让它以这个空间重新加载。这些空间会占用内存，常常比模型本身还多。
- **不能使用工具的模型**仍然可以谈论工程，但无法修改它；Kumi 会说明一次，并列出同一服务器上可以修改的模型。`kumi doctor` 会列出它找到的服务器。

## 连接 Live

Kumi 通过它的桥接访问 Live：桥接由在 Live 内运行的 Remote Script 和由 Kumi 启动的本地 MCP 服务器组成。第一次打开 Kumi 时，以及每当它自带的桥接比 Live 中的新时，Kumi 的设置都会在应用内只完成尚未完成的步骤：

1. **Sign in**：如果 Kumi 还没有登录，先登录。
2. **Connect to Live**：Kumi 把桥接放到位并打开 Live。如果 Live 正开着，选择 **Restart Live now**（Kumi 请 Live 退出，Live 会先提示你保存工作）或 **I'll quit it**（Kumi 等你自己退出 Live）。如果 Kumi 请求后过了一会儿 Live 仍然开着（例如它询问是否保存时你选了 Cancel），Kumi 会说明情况，并提出再请求一次。Kumi 关闭 Live 之后，无论这一步如何结束，Live 都会重新打开。
3. **Control Surface**：第一次时，在 Live 中打开 **Settings → Link, Tempo & MIDI**，把 **AbletonMcpBridge** 选为 Control Surface。Kumi 会自己察觉，Live 也会记住这个选择。

在某一步按 Esc，会把这一步留到以后：Kumi 暂时在不连接 Live 的情况下聊天。Sign in 和 Connect to Live 下次会再次提出；Connect to Live 完成之后，只要 Live 应答，Kumi 就会自己连接。在放置桥接时退出 Kumi，它会等桥接放好后再退出。Live 关闭时，`kumi bridge` 在 shell 中做同样的事：它会请你确认（`--yes` 可预先确认），Live 正在运行时会拒绝执行，并且从不退出或启动 Live。两者都通过桥接自身的生命周期流程安装或更新桥接，带有检查、回执和回滚，并在更新之间保留它的设置和密钥。`kumi bridge` 最多等待十分钟让 Live 连接，连上时会告诉你。当 Live 中的桥接比 Kumi 自带的旧、且 Live 已关闭时，`kumi update` 会替你运行它；需要你自己运行时，`kumi doctor` 会告诉你。

Kumi 会在你的 User Library 中找到 Live 的 Remote Scripts 文件夹，包括你移到别处的 User Library（为此它会读取 Live 自己的设置）。`KUMI_REMOTE_SCRIPTS_DIR` 可以覆盖这个位置。

**Kumi 的 Live 扩展。** 在 Live 12.4 及更高版本上，`kumi bridge` 还会把 Kumi 的扩展放进 Live 的 Extensions 文件夹（macOS 上为 `~/Library/Application Support/Ableton/Extensions/kumi.kumi`；Windows 上 Kumi 使用 `%LOCALAPPDATA%\Ableton\Extensions`，这一路径在 Windows 上尚未确认）。Live 下次打开时会启动它。这个扩展可以把 MIDI 片段直接写进编曲视图、清空轨道上的一段区域、在不播放的情况下渲染轨道的片段，并在 Live 的右键菜单（**Extensions** 下）中加入 **Ask Kumi about this**，它会把你点击的对象附加到你的下一条消息中。开启 Developer Mode（Settings → Extensions）时，Live 不会启动任何扩展，所以由桥接自己启动 Kumi 的扩展。没有这个扩展 Kumi 也能工作，`kumi doctor` 会告诉你它是否已安装、是否在运行。

桥接安装好并在 Live 中选中后，`kumi` 会找到并连接它；无需任何配置。没有桥接时，Kumi 照样启动，在不连接 Live 的情况下聊天（**No Live access**），并提出连接。`kumi --bridge-config <absolute path>` 使用你自己的桥接配置；`kumi --inference-only` 在不连接 Live 的情况下聊天。

**可选：Willington。** 装上单独安装的 Willington provider 后，Kumi 还可以编辑 Follow Actions、映射机架的宏旋钮以及设置链区域（chain zone）。它有适用于 macOS ARM64 上 Live 12.4.15b4 和 b5 的绑定（链区域仅限 b5）；请见[可选的 Willington 集成](WILLINGTON_INTEGRATION.md)。

## 使用 Kumi

Kumi 全屏运行：左边是对话，右边是 Live 面板，底部是输入框。FOCUS 跟随你在 Live 中触碰的对象，NOW 显示 Kumi 正在做什么，HISTORY 列出每项修改及其撤销。窗口宽度不足 100 列时，Live 面板会折叠成输入框上方的一条。设置 `KUMI_UI=plain`，或者把输出通过管道传给其他程序，会改为逐行的纯文本输出，适合屏幕阅读器。详情请见[命令、按键与界面](KUMI_TUI.md)。

试试“描述打开的工程：轨道、速度和走带”，然后问某条轨道上有哪些设备。再请求一项修改，比如“把速度设为 124，并把 3-Audio 重命名为 Bass”：每项修改都会出现在 HISTORY 中，旁边带有 **undo**。点击 FOCUS 中的某个设备，或在 Live 中右键点击某个对象并选择 **Ask Kumi about this**，即可指向它：“这个 Saturator 太刺耳了”。

Kumi 工作时，按 Enter 可以补充说明（它会在当前这一步之后读到你的消息），按 Tab 发送一条等这次回答结束后再处理的消息，`/btw` 可以顺便问个问题而不打断它。Esc 停止这次回答；已完成的步骤会保留。`/stop` 随时停止 Live（片段、走带和录音）。

当 Kumi 请你做选择时（哪条轨道、哪个版本），选项会显示在输入框上方：按选项的数字再按 Enter 回答，或者直接输入你自己的回答。

要给 Kumi 看东西，把文件拖进窗口，或按 **Ctrl-V** 粘贴剪贴板里的图片（比如某个合成器的截图），然后说“做一个这样的”。每个文件都会显示在输入框上方，随下一条消息一起发送；点 ×，或在输入框为空时按 Backspace，即可撤回。模型能看的图片是 PNG、JPEG、GIF 和 WebP，每张不超过 3.75 MB，一条消息合计不超过 20 MB。其他文件，比如参考音频、预设或 Live 工程，会以路径的形式发送，供 Kumi 使用。图片只随它自己的那条消息发送；之后的对话（无论是否保存）会保留每个文件的名称和路径，但不保留图片本身。剪贴板中的图片在 `~/.kumi/attachments` 中保存一周。

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

**Live 不允许脚本做的事：** 把宏旋钮或调制器映射到参数（Willington 可以映射宏旋钮），以及编辑编曲视图的自动化通道。Kumi 会如实说明，并建议变通的办法。保存、导出、冻结、并轨和编组则通过 [Live 自己的命令](#live-自己的命令)完成。

[Kumi 如何修改你的工程](KUMI_CHANGES.md)列出了 Kumi 能做的所有修改。

## 播放、录音与渲染

Kumi 会在你要求时，或者在有助于检查或展示它做出的东西时，播放和停止工程、触发片段和场景、移动播放头以及录音。如果一个计划中途停止（某一步失败，或你按了 Esc），Kumi 会停止它启动的播放和录音。如果 Live 拒绝普通的停止，Kumi 会使用桥接的紧急停止。

Live 不给脚本提供并轨（bounce）功能，所以 Kumi 通过重采样来并轨：它添加一条音频轨道，从源轨道取信号（整个混音则从 “Resampling” 取信号），在编曲视图中录下你要的长度，然后解除该轨道的录音准备。录音会作为音频片段留在工程中。有了扩展，Kumi 还可以在不播放的情况下，把音频轨道自身的片段渲染成文件（取轨道设备之前的信号）。Live 自己的 Bounce to New Track 和 Bounce Track in Place 也可以使用，通过 [Live 自己的命令](#live-自己的命令)。

## Live 自己的命令

有些事 Live 的脚本接口完全没有提供。对于这些，Kumi 会像你一样使用 Live 自己的菜单：编组和取消编组轨道；冻结、解冻和平铺（flatten）；不播放即可并轨（Bounce to New Track、Bounce Track in Place）；合并（Consolidate）；把音频转换成 MIDI（旋律、和声、鼓）；分离音轨（stems）；切片到 MIDI 轨道；保存工程，或收集全部并保存；导出音频或 MIDI 片段。

- Kumi 会选中命令要作用的对象，按下命令，并说明发生了什么变化。Live 打开对话框时（比如导出），Kumi 会读取并回答它。
- 轨道是通过 Live 12 为屏幕阅读器提供的辅助功能按名称选中的。Live 保持原样：不会有任何窗口跳到前面，一条命令用不了一秒（并轨或冻结则取决于 Live 渲染所需的时间）。
- 已经冻结的轨道不会被再次冻结：Live 的命令会把它撤回，所以 Kumi 会先检查。
- 片段命令作用于 Session 中的片段，或你在 Live 中选中的片段。Kumi 目前还不能选中编曲视图中的片段。
- HISTORY 会列出每条命令，Live 自己的撤销（Cmd-Z）可以撤回它。
- **在 Mac 上**这使用辅助功能。第一次时 macOS 会询问：请在 系统设置 › 隐私与安全性 › 辅助功能 中打开运行 Kumi 的应用（你的终端）。`kumi doctor` 会告诉你它是否已打开。**在 Windows 上**它使用 UI 自动化，无需任何设置。

## 聆听

Kumi 能听音频文件和工程中的音频片段：参考曲、采样、并轨结果或录音。它会测量：

- 响度：整合 LUFS、真峰值和响度范围；
- 十个频段的音色平衡（从超低频到空气感频段），以及每个频段的立体声宽度；
- 动态、速度和调性；
- 对单个声音：它的音高、泛音、包络和运动（LFO 的速率，按速度换算）；
- 对带音符的声部：音符本身（取自前一分钟），可以写成 MIDI 片段。

给出参考曲时，它会对齐响度，并说明差别最大的地方。对话中会以小型频谱显示它听到的内容，对比结果则显示为比参考曲高或低多少 dB。Kumi 自己就能读取 WAV 和 AIFF，MP3、M4A、FLAC 等格式则通过 macOS 的 `afconvert` 读取，在其他系统上通过 `ffmpeg` 读取（在 Windows 上，Kumi 会在第一次需要时下载它）。分析在你的电脑上进行：只有数字会发送给模型，音频本身绝不会。

**聆听工程。** 询问某条轨道或混音（“贝斯是不是太浑？”“什么和底鼓打架？”），Kumi 会直接在 Live 中聆听，无需任何设置：

- **Live 正在播放时**，它聆听正在播放的内容几秒钟，不动 Main 和走带。
- **Live 停止时**，它在 Main 静音的情况下播放循环区（或从播放头起的几个小节，或你指定的部分），之后把 Main 恢复原样。
- **同时聆听多条轨道：**每条的声音，以及两条轨道在同一频段以相近电平重叠的地方。
- **原理：**Kumi Ears，一个 Kumi 自带的小型 Max for Live 设备（`kumi bridge` 会把它放进你 User Library 的 Kumi 文件夹）。Kumi 需要聆听时把它放在轨道设备链的末端，用完后移走。声音原样通过，工程中不会录下任何东西。
- 试听（audition）和目标（goal）也用同样的方式聆听候选：不需要临时轨道，也不需要预备录音。没有 Max for Live 时，Kumi 会改为先录音再聆听。

## 匹配参考曲

让 Kumi 把某个东西做得像参考曲（“让贝斯听起来像这个：~/refs/bass.wav”），它会把这当作一次搜索，而不是猜测。它先聆听参考曲，在各自的轨道上搭建两到四个不同的版本，然后在后台把它们一起静默渲染，逐一与参考曲对比打分（0 到 100 分，并列出最大的差别）。接着它精修最好的那个；当没有哪个旋钮能缩小差距时，它会改变结构；在达到目标、新想法不再有帮助，或经过 12 轮或 45 分钟后停止。最后它会给出前后的分数，以及仍然存在的差别。

`/goal` 加上要达到的目标会走得更远：Kumi 会一直搜索，大部分时候用自己的快速旋钮搜索，每隔几代再加入模型更大胆的想法，直到分数达到 95、你让它停下，或过了四个小时。GOAL 标签页显示进展。Esc 暂停目标，单独输入 `/goal` 会继续它（即使重启之后也可以），`/goal stop` 结束它。

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

Kumi 很熟悉十款插件：Serum 2、Vital、Ozone 12、Pro-Q 4、Pro-L 2、Saturn 2、Decapitator、OTT、Supermassive 和 Pigments。处理某个插件之前，它会先读这个插件的指南：它的用途、各个部分、常见声音的配方，以及与插件向 Live 显示的真实参数的对照。

- **用插件自己的单位设定数值：**“800 Hz”“-6 dB”“35 %”，或按名称选菜单项（“Saw”），按插件自己的显示来设定。
- **Live 显示的参数。**Live 只让 Kumi 调节插件中已配置的参数。指南会说明是哪些，以及如何添加更多：点击插件标题栏中的 **Configure**，在它的窗口中把旋钮动一次。Kumi 可以帮你打开插件的窗口。
- **不是参数的东西**（振荡器的波表、滤波器类型、调制路由、Ozone 的 Master Assistant）需要在插件的窗口中完成；指南会说明在哪里。
- **波表。**Kumi 可以用波形和谐波，或从一段声音中截取，为 Serum、Vital 等波表合成器制作波表，放进插件的文件夹；你把它拖到振荡器上即可。

其他插件也能按参数名称使用。

## 观看视频教程

给 Kumi 一个视频，让它搭建视频里展示的内容。视频可以是 YouTube 教程（或任何 [yt-dlp](https://github.com/yt-dlp/yt-dlp) 能读取的网站上的视频），也可以是你电脑上的视频文件：

> 看看这个，在新轨道上做出这个贝斯：https://www.youtube.com/watch?v=…

Kumi 会读取视频的标题、章节和文字内容。文字来自视频的字幕；没有字幕时，则来自视频里的语音，在你的电脑上转写。然后它会查看旁白提到某个设备、设置或数值时的画面，需要读取数值时还会放大查看。接着它用几行话说明这个视频做了什么，并在你的工程中搭建出来。视频用到了你的工程里没有的东西时（某个插件、某个采样），Kumi 会说明，并改用 Live 中最接近的设备。

所需工具：

- **yt-dlp**：Kumi 会在第一次时把它下载到 `~/.kumi/tools`（约 35 MB，按其发布版本的校验和验证），之后每月更新一次。 对于需要执行 JavaScript 的视频，Kumi 会优先复用旧安装版保留的 Node，其次查找 PATH 中的 `node`，并传给 yt-dlp。全新原生安装不会附带这个可选运行时；部分 YouTube 视频需要它。
- **ffmpeg**：用于画面和声音。在 Mac 上运行 `brew install ffmpeg`；在 Windows 上，Kumi 会在第一次时下载它（约 170 MB，经过校验）。没有它时，Kumi 只能读取视频的文字内容。
- **whisper.cpp**：只用于没有字幕的视频。在 Mac 上运行 `brew install whisper-cpp`；在 Windows 上由 Kumi 下载。它的语音模型（约 190 MB）会在第一次时下载。

`kumi doctor` 会告诉你是否已有 ffmpeg 和 whisper.cpp。视频不会被完整下载：画面和声音取自视频流中 Kumi 要看的那些时刻。最近 24 个视频保存在 `~/.kumi/videos` 中，所以再看同一个视频会很快。视频的文字和画面对 Kumi 来说只是信息，绝不是给它的指令。

## 制作 Max for Live 设备

用你自己的话请求一个 Live 没有的设备，Kumi 就会把它做出来，并放到你的轨道上：

> 做一个 MIDI 效果器，只保留每个和弦中最低的音，然后把它放到 Keys 轨道上

> 给我做一个听起来像 Erbe-Verb 的音频效果器

你不会特意说明的细节由 Kumi 来决定，它会告诉你它选了什么。当设备应该像某个现有设备那样工作时，它会先查明原版是怎么工作的。设备会放进你 User Library 中的 Kumi 文件夹，Live 的 Browser 会像列出其他设备一样列出它；旋钮数量按需而定（最多三排）。这些旋钮都是普通的 Live 参数，所以你可以为它们写自动化、做映射。加载设备是 HISTORY 中的一项修改，带有撤销。

MIDI 效果器用 JavaScript 编写。在做出设备之前，Kumi 会在你的电脑上用它编写的测试和它自己的检查（没有错误、每个音符都会被释放、没有遗留仍在运行的东西）来运行代码，不通过就修复。音频效果器或乐器用 GenExpr 编写，这是 Max 的 gen~ 所用的语言。效果器带有 Mix 和 Output 旋钮；乐器可同时发 8 个音，最多 32 个。两者最后都经过 Kumi 的输出级，以确保输出安全（没有 NaN、非正规数或直流偏移，并保持在 +6 dBFS 以下）。Kumi 会聆听它做出的东西，并修正它听到的问题。

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

## Kumi 会记住什么

Kumi 保存的所有内容都会在保存时显示出来：对话中的一行，以及 HISTORY 标签页顶部带 **forget** 的一行。`/memory` 列出全部内容；选择某一项即可让它忘掉，选择一条笔记还可以修改它的文字或将它置顶。

- **笔记**（✎）：你告诉 Kumi、而 Live 无法显示的内容，比如某条轨道的用途、你想要的效果、你的习惯和喜好。关于你的笔记保存在 `~/.kumi/memory.json`；关于某个已保存工程的笔记保存在 `~/.kumi/projects` 中该工程的文件夹里。每处最多 24 条，每条一句话；满了之后，未置顶的笔记中最旧的一条让出位置。Kumi 不会保存工程本身就能显示的内容、它自己做过的事，或任何读起来像指令、看起来像密钥的内容，因此工程里的文字无法变成长期指令。
- **技巧**（◆）：Kumi 做出的某个东西背后的思路，保存下来以便日后用于同类声音。当某次构建值得复用时，Kumi 会在回答之后问你是否保存：按 1 保存，按 2 不保存；不回答就继续往下做也不会保存（在下一条消息里说“保存”同样可以）。撤销这次构建，这个问题也随之撤回。它只针对你要求的构建发问，从不针对 `/goal` 中的工作，而且每三次回答最多问一次。你给出的教程、参考或步骤始终优先：只有当你没有指定做法时，Kumi 才会用到某个技巧，并且会告诉你它在用。最多 40 条，保存在 `~/.kumi/techniques.json`。
- **配方**（↻）：可以在任何工程中重放的工作方式，比如人声效果链或重采样循环。让 Kumi 保存它刚做的事、描述一个固定流程，或者说“看我做”，然后在 Live 中手动操作，做完时告诉它：Kumi 会把发生的变化变成一个配方，每次不同的地方留作空白。按名称请求某个配方即可运行它，或者使用 `/recipes`。配方保存在 `~/.kumi/recipes` 中，每个配方一个文件。
- **经验**（✦）：Kumi 在匹配声音时学到的东西，供下一次匹配使用。最多 60 条，保存在 `~/.kumi/playbook.json`。

当某个请求需要 Kumi 的工具或 Live 的脚本接口没有提供的功能时，Kumi 会告诉你，给出变通的办法，并把缺失的能力记录在 `~/.kumi/gaps.jsonl` 中，供 Kumi 的开发者参考。这个记录绝不会被读回对话中；当你选择发送 `kumi report` 时，报告中会包含它。

Kumi 还会把每次回答的时间花在哪里（模型、工具、对 Live 的请求、发送的字节数）记录在 `~/.kumi/timings.jsonl` 中，保留最近大约 1000 次回答。和缺失功能记录一样，它只留在你的电脑上，只有在你发送 `kumi report` 时才会带出去。

## 对话与离开期间的变化

Kumi 把每个已保存工程的对话保存在 `~/.kumi/projects` 中：每次回答后都会保存，每个工程保留最近 20 个对话，每个最多约 256 KB（最早的交流会被丢弃）。在已保存的工程上打开 Kumi，会接着它最近的对话继续，HISTORY 中显示最近 100 项修改（不带撤销）。`/new` 开始新的对话，并保留上一个；`/conversations` 可以回到其中任何一个。未保存工程的对话会在你第一次保存时移到该工程自己的文件夹。

当你提到以前的事（“上周那条人声的混响链”）时，Kumi 会按字词查找为每个工程保存的对话，以及它保存的技巧和配方。搜索在你的电脑上进行；找到的内容会像其他读取结果一样发给模型。

Kumi 还会记住它最后一次看到的每个已保存工程是什么样子。下次打开时，欢迎界面会说明这期间发生了哪些变化（“Since you were last here · 3 days ago: Tempo 120 → 124 BPM; Added track “Pad””），Kumi 也会把这些考虑进去。工程是按文件路径识别的，所以 Save As 之后会重新开始。

## 当 Live 断开时

Live 关闭或崩溃时，Kumi 会在一秒内察觉，告诉你，并保留对话。它每两秒查找一次 Live，Live 回来后会自动重新连接；正在执行的请求会回到输入框中，按一下 Enter 即可重新发送。如果 30 秒后 Live 仍未回来，Kumi 会问你 Live 是否已打开、是否已把 AbletonMcpBridge 选为 Control Surface。`/reconnect` 会立即尝试重连。

Kumi 的撤销只在它与 Live 的连接持续期间有效：在 Live 重启、重新连接或 Kumi 重启之后，之前的修改会显示 **no undo**，只能用 Live 自己的撤销来撤回。`/new` 会保留连接，所以撤销仍然可用。

## 更新、报告与卸载

```sh
kumi update              # 获取最新的 Kumi；Live 中的桥接较旧时一并更新
kumi update --check      # 只告诉你是否有更新的 Kumi
kumi update --rollback   # 回到上次更新之前的 Kumi
kumi doctor              # 检查 登录、桥接、Live、扩展和终端
kumi report              # 出问题时生成一个可以发送的文件
kumi uninstall           # 卸载 Kumi；加上 --all 会同时删除你的对话、笔记和登录信息
```

`update` 会获取最新版本，按其校验和验证，并先启动一次以确认它能运行，然后才把它放到位；之前的版本会保留，供 `--rollback` 使用。如果 Live 中的桥接较旧且 Live 已关闭，它接着会运行 `kumi bridge`；如果 Live 正开着，它会告诉你退出 Live 再运行 `kumi bridge`。在仓库的副本中，`update` 改为把检出向前推进（`git merge --ff-only`，有本地修改时拒绝执行），并用 Cargo 构建工作区。在 Kumi 中，`/update` 会先询问你，然后关闭 Kumi、更新，再用同一个对话重新打开它。

从当前的 1.7.5 安装版（自带 Node 24）升级时，关闭 Live，运行 `kumi update`，再照常打开 Kumi。原生应用首次启动时会迁移桥接，即使新旧桥接都为 1.0.74。设置、登录信息、对话和素材库仍在 `~/.kumi` 或已有的 `KUMI_HOME` 中，无需重新登录或移动数据。

上一版应用及其 Node 会保留供回滚。在 Windows 上，原生版首次启动时会替换旧启动器，之后的启动不再经过 Node；普通更新不需要额外操作。`kumi update --rollback` 会一起恢复 JavaScript 应用及其桥接配置和密钥；必须先关闭 Live，并保留桥接的上一代文件。再回滚一次就会返回原生应用。Kumi 从不自行关闭 Live。

很早的 Node 22 安装版可能因 Node 版本而拒绝更新。出现这种提示时，请保留原来的 `KUMI_HOME` 并重新运行[安装程序](../../README.zh-CN.md#快速开始)，它会保留数据并安装原生应用。npm 用户仍可使用原来的命令：有 Cargo 时构建当前检出的代码，没有时迁移到对应的已发布原生版本。

Kumi 会在启动时检查是否有新版本，每天最多一次；没有新版本或没有网络时什么也不说。在 `~/.kumi/settings.json` 中加入 `"updateCheck": false`，或者设置 `KUMI_NO_UPDATE_CHECK=1`，即可关闭这项检查。

`report` 会写出 `~/kumi-report-<date and time>.txt`，其中包含 Kumi 和桥接的版本、doctor 的检查结果、你的设置、Kumi 在你上一次对话中做了什么、缺口日志，以及 Live 自身日志中来自桥接的行。密钥和令牌会被删除，你的主文件夹显示为 `~`，你的账户名显示为 `<user>`。发送前请先读一遍。

`uninstall` 会移除 Kumi、保留的旧版 Node、启动器以及它添加的 PATH 条目，并提出把桥接和扩展从 Live 中移除（仅在 Live 关闭时）。你的对话、笔记、配方和登录信息会保留，除非你加上 `--all`。

## 限制

| 项目 | 限制 |
| --- | --- |
| 你发送的一条消息 | 16 KiB |
| 一次回答 | 200 个模型步骤；10 分钟没有进展，或总计 60 分钟后停止（匹配和目标的时间更长） |
| 重试 | 提供方出错时最多重试 3 次，前提是该步骤还没有显示任何输出 |
| 发往桥接的一个请求 | 65 秒 |
| 一次回答中的修改 | 5,000 项（一批打击垫或参数只算一次） |
| 计划中的 `wait` 步骤 | 30 分钟 |
| 对话大小 | 超过约 160 KB 时，较早的 Live 读取结果会被压缩；超过约 400 KB 时，最早的交流会被丢弃 |
| 能听的音频 | 文件的前 12 分钟 |
| 视频转写 | 每次 90 分钟 |
| 事先检查的可用磁盘空间 | 录音需要 100 MB（在工程所在的磁盘上；未保存的工程则在你的主文件夹所在的磁盘上），制作设备需要 100 MB（在 User Library 所在的磁盘上） |

**聆听**听的是文件、录音以及工程的轨道和混音（工程通过 Kumi Ears 聆听，需要 Max for Live）。它负责测量和比较，不评判品味。

**Live 自己的命令**在 Mac 上需要为你的终端打开辅助功能。片段命令作用于 Session 中的片段，或你选中的片段。

**你电脑上的模型：**服务器没有运行、没有某个模型、模型对可用内存来说太大，或者窗口对 Kumi 的指令和工具来说太小，都会连同解决办法一起说明（`ollama serve`、`ollama pull <model>`、更小的模型、更大的上下文），并提议重新发送消息或换一个模型。Kumi 的工具中的图片只会以文字形式传给本地模型。

**观看视频**取决于各个网站当前的情况。私密视频、仅限会员的视频以及部分有年龄限制的视频无法读取，自动生成的字幕也可能听错名称。不能接收图片的模型只能得到视频的文字内容。

**桥接版本。** Kumi 在连接时读取桥接的版本，只提供该版本支持的工具。[桥接版本](KUMI_CHANGES.md#桥接版本)说明了哪些工具需要哪个版本。

## 隐私：哪些内容会离开你的电脑

- **你的模型提供方**会收到你的消息、对话、Kumi 从工程中读取的内容、它观看的视频画面、它读取的图片，以及你随消息添加的图片。使用你电脑上的模型时，这些内容不会离开你的电脑（写在 `settings.json` 中的服务器无论运行在哪里都会收到）。
- **网络搜索和阅读**会发送到上面提到的搜索服务，Kumi 读取的网页也会看到它的请求。Kumi 不会读取带有密钥或令牌的地址。
- **下载**来自 GitHub（Kumi 的发布版本和更新检查、yt-dlp、ffmpeg 和 whisper.cpp）、Hugging Face（语音模型）以及你指定的视频网站。
- **音频**在你的电脑上分析；只有数字会发送给模型。
- **你的声音**在你的电脑上转写，录音随即删除；只有你发送时的文字会离开。
- **你的素材库**在你的电脑上学习；只有手册的页面来自网络。

轨道、片段和设备的名称、工具结果以及网页对 Kumi 来说都是数据，绝不是指令。Kumi 把它的文件保存在 `~/.kumi` 中，只有你可以读取；登录信息保存在 `~/.kumi/auth.json`。终端的滚动记录，以及提供方自己的数据保留，不在此范围内。

## 文件与设置

Kumi 把一切都保存在 `~/.kumi` 中。`~/.kumi/settings.json` 包含：

| 键 | 含义 |
| --- | --- |
| `model` | 所选模型，`<provider>/<model>`（`/model`、`kumi model`） |
| `effort` | `low`、`medium`、`high`、`xhigh` 或 `max`；不设置时使用模型的默认值（`/effort`） |
| `panelTab` | 你上次打开的 Live 面板标签页 |
| `updateCheck` | `false` 关闭启动时的新版本检查 |
| `modelServers` | 兼容 OpenAI 的模型服务器：`[{ "name", "baseURL", "apiKey" }]`（[你电脑上的模型](#你电脑上的模型)） |
| `libraryFolders` | 供 Kumi 学习声音、预设和工程的其他文件夹 |
| `voice` | 说话设置：`send`（停止后直接发送）、`language`、`microphone`（`/voice`） |

环境变量（路径必须是绝对路径）：

| 变量 | 含义 |
| --- | --- |
| `KUMI_MODEL` | 本次运行使用的 `<provider>/<model>`，覆盖所选模型 |
| `KUMI_AUTH_FILE`、`KUMI_SETTINGS_FILE` | 登录信息存储和设置文件 |
| `KUMI_MEMORY_FILE`、`KUMI_TECHNIQUES_FILE`、`KUMI_PLAYBOOK_FILE` | 关于你的笔记、技巧和经验 |
| `KUMI_RECIPES_DIR`、`KUMI_PROJECTS_DIR`、`KUMI_GOALS_DIR` | 配方；每个工程的对话、笔记和最后状态；进行中的目标 |
| `KUMI_INPUT_HISTORY_FILE`、`KUMI_GAPS_FILE`、`KUMI_RESTORE_FILE` | 你发送过的内容（供 ↑ 调出）、缺口日志，以及渲染中途崩溃后要恢复的 Main 音量 |
| `KUMI_VIDEOS_DIR`、`KUMI_TOOLS_DIR` | 看过的视频，以及 Kumi 下载的程序 |
| `KUMI_LIBRARY_DIR` | Kumi 学到的你的声音、预设和工程 |
| `OLLAMA_HOST`、`LM_API_TOKEN` | Ollama 的监听地址（按 Ollama 自己的读法）；LM Studio 的服务器需要时的 API 令牌 |
| `KUMI_EARS=0` | 不用 Kumi Ears，改为录音后再聆听工程 |
| `KUMI_FAST=0` | 通过桥接的预览和应用这条较慢的路来设置设备参数（[一项修改如何进行](KUMI_CHANGES.md#一项修改如何进行)） |
| `KUMI_YTDLP`、`KUMI_FFMPEG`、`KUMI_WHISPER`、`KUMI_WHISPER_MODEL` | 按路径指定你自己的 yt-dlp、ffmpeg、whisper.cpp（`whisper-cli`）或语音模型（`ggml-*.bin`） |
| `KUMI_REMOTE_SCRIPTS_DIR` | Kumi 找不到 Live 的 Remote Scripts 文件夹时，用它指定 |
| `KUMI_LIVE_EXTENSIONS_DIR` | Kumi 找不到 Live 的 Extensions 文件夹时，指定 `kumi bridge` 放置 Kumi 扩展、`kumi doctor` 查找扩展的位置 |
| `KUMI_BRIDGE_WAIT_SECONDS` | `kumi bridge` 等待 Live 连接的时长；`0` 表示不等待 |
| `KUMI_NO_UPDATE_CHECK` | 设为任意值即关闭启动时的新版本检查 |
| `KUMI_UI=plain` | 用逐行的纯文本输出代替全屏应用；`kumi bridge`、`kumi update` 等命令在运行时也不显示加载动画 |
| `KUMI_COLOR` | 颜色检测出错时设为 `truecolor`、`256`、`16` 或 `none`；也会遵循 `NO_COLOR` |
| `KUMI_ICONS` | 终端里的符号显示不正常时设为 `glyphs` 或 `badges`（两个字母的图标） |
| `KUMI_TRACE=1` | 打印每次桥接调用的名称（不含参数和结果） |

安装程序会读取 `KUMI_HOME`（安装到 `~/.kumi` 以外的位置；须在安装时设置，而不是之后）、`KUMI_VERSION`（安装指定的版本）、`KUMI_RELEASES`（从哪里下载；它优先于 `KUMI_VERSION`）和 `KUMI_NO_MODIFY_PATH=1`（不修改你的 PATH）。
