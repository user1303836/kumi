# Kumi 的命令、按键与界面

[English](../en/KUMI_TUI.md) · 简体中文 · [日本語](../ja/KUMI_TUI.md)

Kumi 终端应用的参考：屏幕上显示什么、每条命令和按键，以及纯文本模式。[指南](KUMI_GUIDE.md)介绍 Kumi 能做什么。

## 界面

Kumi 占据整个终端窗口，并在 Kumi 关闭、崩溃或被停止时把窗口恢复原样。

- **顶栏：** Kumi、工程名称、模型及其推理强度、正在运行的后台任务数（◐ 2 jobs），以及与 Live 的连接状态。Live 播放时，速度旁边有一盏黄灯随节拍闪烁（每小节第一拍最亮）。备用模型回答期间，顶栏显示它的名字并加上“· fallback”；接近套餐上限时（某个窗口已用 80%），模型旁边会显示“plan 84% · resets 14:20”。模型的上下文用到一半以上后，模型旁边会有一个计量条显示用了多少（“context ▰▰▰▰▰▰▱▱▱▱ 62%”，从 80% 起显示为警告色）。
- **对话**（左侧）：你的消息、Kumi 的回答及其步骤，用平常的话描述（“looked at your Set”），每条都附有耗时。正在进行的步骤按类型显示动画（搜索、阅读、构建设备、修改、聆听、播放）；同一步骤做了多次会折叠成一行（“read a page ×3”）。Kumi 听到的内容显示为一个小频谱，它记住的内容单独占一行：✎ 笔记，◆ 技巧，↻ 配方，✦ 从匹配中得到的经验。模型在步骤之间思考时，它推理的标题会像进行中的步骤一样单独占一行。每个完成的回答最后都有一行不显眼的文字：耗时、修改数，以及使用 API 密钥时的 token 数或使用订阅时占套餐的比例（“6.2s · 2 changes · 18.4k in · 1.2k out”）。
- **FOCUS**（右上）：你在 Live 中所处的位置，随你的选择变化而更新：轨道的设备树（含机架链）、以小型钢琴卷帘显示的片段音符，或 Session 视图、编曲视图的条带。你指向的设备（点击它，或在 Live 中使用 **Ask Kumi about this**）会被固定，用于你接下来的消息。
- **NOW**（右侧中部）：Kumi 正在做的事，随进行实时绘出：某个值修改前后的样子、EQ 或压缩器的曲线、自动化包络线、新片段的音符、色块、落入设备链的设备、“▶ Playing from the start marker”。Kumi 等待模型时，会说明原因和要等到何时：“retrying in 5s · ChatGPT is busy”、“carrying on at 14:20 (in 2h 13m) · ChatGPT's plan limit”、“offline · trying again in 20s”（都可用 Esc 停止）。模型思考时，NOW 会显示已思考多久（“thinking · 31s”）和它推理的标题：只有一行，从不显示推理本身，推理也不会进入回答或 `/copy`。
- **标签页**（右下）：**HISTORY** 先列出 Kumi 最近记住的三项内容（各带 **forget**），然后是每项修改，最新的在前，各带 **undo**；无法撤销时则显示 **kept** / **no undo** / **check Live**。展开一项修改即可看到它的图示：EQ 曲线、压缩器曲线、音符（新增的为彩色，删除的为淡色）、自动化包络线，或每个值修改前后的对比；当 Live 设置的值与所要求的不同时，也会标出所要求的值（“asked -2 dB”）。**GOAL** 显示 `/goal`（设定之前显示“No goal yet”）：目标、已完成的回合数和用时与预算的对比（状态栏计的是进行中的回合）、最后一次检查以及下一步；匹配声音的 `/loop` 则显示最佳得分及其趋势、领先的候选方案，以及用时。第一个后台任务开始后会多出 **JOBS**：按最新在前列出每个任务的进度、用时和 token，各带 **stop**；按 Enter 打开它的步骤和报告。
- **输入框**（左下）：等待发送的消息显示在它上方，被固定的设备显示为一个小标签，随下一条消息发送的文件则显示名称、类型、大小和 ×。为空时显示 “ctrl+t to talk”。Kumi 聆听时，它的底行会显示一个闪烁的薄荷绿 `●`、时间和电平表，右侧是可用的按键；不用红色，因为在 Live 中红色表示录音。

欢迎界面会显示自你上次使用以来工程中发生的变化；有更新的 Kumi 时也会显示；第一次时还会说明 Kumi 正在后台学习你的素材库。

更新后第一次打开 Kumi 时，对话开头会显示 **What's new**（新功能）：从 Kumi 的更新日志中取最新的最多五条，其余的用 `/changelog` 查看。如果有从上次接着的对话，新功能会显示在它下面。全新安装时不显示，模型也看不到这些内容。在 `~/.kumi/settings.json` 中写入 `"whatsNew": false` 即可关闭。

接着进行的对话会按你离开时的样子回来：最后 30 个请求连同它们的步骤、图示和试听轮次都会显示，上方注明更早还有多少条消息没有显示。HISTORY 中会列出它的更改，但重启后 Kumi 无法撤销（DAW 自己的撤销仍然可用）。旧版 Kumi 保存的对话照旧只以文字形式回来。

**设置**会在还有步骤未完成时最先出现：登录、Live 中 Kumi 的桥接（缺失，或比 Kumi 自带的旧），然后是 Control Surface。它列出这三步，已完成的以薄荷绿显示并注明所选内容，并询问当前这一步需要什么；Kumi 等待时（等浏览器、安装或 Live），字标下方的薄荷绿波形会动起来。↑↓ 和 Enter 用于选择，Esc 把这一步留到以后，之后的会话在同一窗口中显示。等待 ChatGPT 登录时，**c** 复制链接（或代码），**d** 改用代码而不用浏览器，**b** 改回浏览器。在 Linux 上，设置只进行登录。已经设置好的 Kumi 会直接进入会话。

宽度不足 100 列时（放在 DAW 旁边的窗口），窗格会变成对话下方的**抽屉**：一行标签页（NOW、FOCUS、HISTORY 等），下面是当前显示的标签页，高度随窗口在 6 到 10 行之间。

- Kumi 做出修改时，抽屉会打开到 NOW：Kumi 正在做的事、这次修改及其撤销，以及它的图示。目标会把收起的抽屉打开到 GOAL。Kumi 打开的抽屉在 10 秒内没有新动静时，会收回到那一行标签页。
- 点击标签页即可在那里打开，点击当前显示的标签页则会关上它；Shift-Tab 和 Tab 进入标签页或 FOCUS 的设备树时，也会把抽屉打开到那里。你自己打开的抽屉会停在你选的标签页上一直开着，直到你关上它。它从不抢走输入框的键盘。
- Ctrl-\\ 打开或关闭抽屉。你关上之后，Kumi 会让它保持关闭，直到你再次打开。

不足 24 行时，窗格改为输入框上方的两行条带（先是你所在的位置和 Kumi 正在做的事，然后是最近一次修改及其撤销）。窗口小于 24×8 时，Kumi 会请你把窗口调大。

## REAPER 与 Renoise

在 REAPER 和 Renoise 中界面也一样（[如何连接](KUMI_GUIDE.md#连接-reaper-或-renoise)），用的是该 DAW 自己的说法和视图。

- **标题栏：** 工程或歌曲的名称，以及与 DAW 的连接（"● REAPER"；被对话框或渲染占住时显示 "REAPER busy"）。节拍灯跟随 DAW 自己的时间闪烁，并在速度旁显示拍子：REAPER 是经由速度图得出的拍号（"7/8"），Renoise 是每拍行数（"LPB 4"）。
- **REAPER 的 FOCUS** 跟随 REAPER 中拥有焦点的部分：
  - **编排视图**：所选轨道前后的轨道，按文件夹层级缩进（完全折叠的文件夹会隐藏其中的轨道），对象（item）显示为条块，还有标记与区域、循环区间和播放头；
  - **MIDI 编辑器**：take 的音符显示为小型钢琴卷帘，下方最多三条控制器轨道（CC、弯音、音色切换、通道压力、复音触后），以及 "and n more lanes"；
  - **FX 链**：以树状显示，输入 FX 自成一枝，容器展开显示其中的 FX，旁通、离线和并联的 FX 会有标注；take 的 FX 拥有焦点时显示 take 的 FX 链（"Kick · take “kick.wav”"）；
  - 轨道的**路由**窗口：它的发送、接收和硬件输出及其电平，以及 MIDI 硬件输出；
  - **混音器**窗口（未停靠的；停靠的混音器无法分辨）：主控和所选轨道前后的轨道，显示音量、声像、静音、独奏、录音预备和峰值表。
- **Renoise 的 FOCUS** 跟随中间的主框架和各个面板（Renoise 不告诉 Kumi 键盘焦点在哪里，所以 FOCUS 按发生变化的部分来跟随）：
  - **Pattern Editor**（模式编辑器）：光标附近的行（播放且开启 Follow Player 时则是播放位置附近），以 Renoise 的记法显示每条轨道的音符、乐器、音量、声像、延迟和效果列，轨道名用轨道颜色显示；
  - **Pattern Matrix**（模式矩阵，显示时）：序列中每个槽位的模式、段落和静音的轨道，以及当前槽位；
  - **Phrase Editor**（乐句编辑器）：同样的网格，附上乐句的 LPB、键位跟踪和循环；
  - **乐器**（Instrument Editor 或 Sample Editor）：键区（keyzone）显示为琴键图，以及每个采样的静音组和切片、宏、乐句、调制组和 FX 链；
  - **Track DSPs**（该面板打开时）：以树状显示效果链，Doofer 连同其中的设备，Splitter 每条通道一枝（Frequency Low 与 High、Mid 与 Side、Parallel Lane 1 与 2），旁通的设备会有标注；
  - **Mixer**（混音器）：每条轨道效果后的音量、声像、静音和独奏（没有电平表：Renoise 的 API 不提供）。
- **指向：** 在 FX 和 DSP 树中，Tab 和点击与在 Live 的设备树中一样可用。
- **图示：** NOW 和 HISTORY 中每类修改各有一种图示：模式或乐句的行在修改前后的样子、包络或自动化的点、变动的参数（带名称和条形）、路由的数值（发送、接收、硬件输出）、键区、轨道上的对象、设置的前后，以及载入效果链后轨道上的设备。键区和对象以图画显示（终端能显示图片时为图片）；行和路由以文字显示。
- **设置**在那里只有两步，在 Linux 上也一样：登录，以及 Connect to REAPER（或 Renoise）。没有 Control Surface。
- **命令**使用该 DAW 的说法：`/refresh` 重新读取你的 REAPER 工程，`/stop` 停止 REAPER 的走带和录音，以及 Kumi 正在大声播放的内容。`/willington` 只属于 Live。
- **离开 Kumi 时：** Kumi 的那一行显示在 Renoise 的状态栏中。REAPER 没有 Kumi 能写入的状态栏，所以用通知告诉你；`/status` 会说明是哪一种。

## 终端中的命令

以 `kumi <command>` 运行；在仓库副本中，以 `npm run kumi -- <command>` 运行。

| 命令 | 作用 |
| --- | --- |
| `kumi` | 在你的 DAW（Live、REAPER 或 Renoise，在 Kumi 之前或之后打开都可以）当前打开的内容上打开 Kumi；第一次时登录，并把 Kumi 的桥接装入那个 DAW（Kumi 可以替你重启 Live）。Kumi 未连接时打开另一个 DAW，Kumi 会跟过去 |
| `kumi --daw live`、`kumi --daw reaper`、`kumi --daw renoise` | 不论开着哪个 DAW，都只用那个 DAW；第一次时登录，并装入 Kumi 的桥接。`KUMI_DAW` 作用相同；`kumi --daw reaper doctor`（或 `report`）说的是那个 DAW |
| `kumi --daw reaper bridge [--yes]`（或 `renoise`） | 把 Kumi 的扩展装入 REAPER，或把它的工具装入 Renoise，或把它们更新到最新；`--yes` 预先回答它的提问 |
| `kumi -c`（或 `--continue`） | 接着你上次使用的对话继续，不论它属于哪个工程 |
| `kumi -r [ID 或名称]`（或 `--resume`） | 按 ID（或其开头）或名称继续一段保存的对话；不加参数则从列表中选择。两者都可与 `--inference-only` 一起使用 |
| `kumi export [ID 或名称 \| --last] [--md \| --html \| --json] [--full] [--raw] [-o 路径]` | 将保存的对话存为 Markdown（默认）、网页或供 Kumi 使用的文件；不写 ID 或加 `--last` 时为最近的对话。`--full` 加上每个步骤的输入和结果，`--raw` 保留密钥和路径 |
| `kumi import <文件>` | 导入用 `kumi export --json` 保存的对话，之后可用 `kumi -r` 继续 |
| `kumi --inference-only` | 不连接 DAW，只聊天 |
| `kumi -p "<请求>" [--json]`（或 `--print`） | 执行一个请求，输出回答后结束，供脚本使用。出错的信息输出到 stderr，退出码表示结果：0 已回答，1 失败，3 停在单次回答的步数上限，130 被 ctrl+c 停止。加 `--json` 则改为把 Kumi 发出的每个事件按行输出为 JSON。固定短语（如“tempo 124”）不经模型，立即修改 DAW。连接 DAW 时，请求会像 `kumi` 一样进入当前打开工程的对话。可与 `--inference-only`、`--daw`、`-c` 和 `-r <ID 或名称>` 一起使用 |
| `kumi --bridge-config <absolute path>` | 使用你自己的桥接配置 |
| `kumi login` | 登录：询问使用 ChatGPT 还是 API 密钥 |
| `kumi login <provider>` | 登录 `openai-codex`（ChatGPT；没有浏览器时加 `--device`，通过 SSH 时默认如此；要用浏览器则加 `--browser`），或用 API 密钥登录 `anthropic`、`openai`、`opencode` 或 `opencode-go` |
| `kumi logout <provider>` | 移除 Kumi 在该提供方的登录 |
| `kumi model [<provider>/<model>]` | 显示或选择模型，也可以是你电脑上的模型（`ollama/<model>`） |
| `kumi auth` | 哪些提供方可用、当前模型以及登录文件（从不显示机密信息） |
| `kumi bridge [--yes] [--allow-dirty]` | 在 Live 关闭时：把桥接装入 Live，或将其更新到最新。`--yes` 确认 Live 已关闭（没有终端时，不加它就什么都不改）；`--allow-dirty` 允许带有未提交修改的检出副本安装其桥接 |
| `kumi doctor [--json]` | 检查安装（版本、回滚、PATH、更新、可用空间、Live、用户库、桥接的端口、Developer Mode）、登录、你电脑上的模型服务器、桥接、Live、扩展、素材库、视频程序、说话功能、Live 的菜单、终端，以及你在用的 REAPER 和 Renoise；告诉你该运行什么。`--json` 输出时不含密钥和主文件夹 |
| `kumi library [--rebuild]` | Kumi 学习你的声音、预设和工程进行到了哪里；`--rebuild` 全部重新学习 |
| `kumi update [--check \| --rollback \| --version <x.y.z>]` | 获取最新的 Kumi（桥接较旧时也一并更新，REAPER 的扩展和 Renoise 的工具换成它附带的版本）；`--check` 只检查；`--rollback` 回到上一个版本（Live 已关闭时连同其桥接）；`--version` 安装那个版本，较新或较旧都可以（仅限已安装的 Kumi） |
| `kumi report` | 写出 `~/kumi-report-<date and time>.txt`，出问题时把它发给我们 |
| `kumi uninstall [--all] [--yes] [--dry-run]` | 卸载已安装的 Kumi 及其下载的内容；`--all` 同时删除对话、笔记、配方、登录信息、素材库和参考曲目；`--yes` 跳过第一个确认问题；`--dry-run` 只说明会删除什么 |
| `kumi --version`（或 `-v`）、`kumi --help`（或 `-h`、`help`） | 版本；帮助 |

## Kumi 内的命令

输入 `/` 打开这些命令的菜单：↑↓ 选择，Tab 补全，Enter 运行，Esc 关闭。以路径开头的消息（拖进终端的文件）是一条消息，而不是命令。

| 命令 | 作用 |
| --- | --- |
| `/new [name]` | 开始新的对话（可以命名）。之前的内容仍留在屏幕上，以一条分隔线隔开；上一段对话会被保存，HISTORY 中的撤销依然有效 |
| `/btw <question>` | 随时顺便问个问题，即使 Kumi 正在工作也可以：根据目前为止的对话回答，不使用工具，显示在一个面板中，且不加入对话。只输入 `/btw` 会再次显示上一个回答 |
| `/resume [ID 或名称]` | 保存的对话：本工程的，按 Tab 显示所有工程的；选择一段即可继续。给出 ID（或其开头）或名称时，直接继续那一段。`/conversations` 相同 |
| `/rename [name]` | 为这段对话命名；不加名称时，当前名称会放进输入框供你修改 |
| `/fork [name]` | 用新的 ID 在这段对话的副本中继续；原对话保留 |
| `/export [copy \| md \| html \| json]` | 将这段对话存为文件：复制为 Markdown，或保存为 Markdown、网页、供 Kumi 使用的文件（`kumi import`）；不加参数时可以选择。超过 64 KB 时改为保存文件 |
| `/share` | 将这段对话保存为随处可打开的网页（没有变化时仍是同一个网页），然后打开它、在访达（资源管理器）中显示它，或复制它的路径，直到选择 Done；通过 SSH 连接时，会告诉你网页在哪里 |
| `/reconnect` | 通过新的桥接重新连接 Live，保留对话（之前的修改将失去 Kumi 的撤销） |
| `/daw [name]` | 显示 Kumi 正在使用哪个 DAW；`/daw live`、`/daw reaper` 或 `/daw renoise` 会关闭 Kumi，并用那个 DAW 重新打开。在你使用 REAPER 或 Renoise，或其中之一装有 Kumi 的桥接后显示 |
| `/undo` | 撤销 Kumi 最近一次修改 |
| `/takeback [n]` | 收回你最后一个请求（或最后 n 个）：Kumi 根据自己的记录，从最新的开始在 DAW 中撤销它们的更改，对话回到这些请求之前，请求的文字会回到输入框，可以修改后重新发送。Kumi 上次启动之前的更改保持原样；如果某次撤销失败（比如你之后改动过它），Kumi 会停在那里并说明是哪一个 |
| `/stop` | 停止 Live：片段、走带和录音。同时停止 Kumi 的回答 |
| `/refresh` | 重新读取工程，不询问模型 |
| `/copy` | 把 Kumi 的上一个回答复制到剪贴板 |
| `/model`、`/effort` | 选择模型（来自各提供方自己的列表，然后是你电脑上的模型服务器：Ollama、LM Studio 以及 settings.json 中写的服务器；输入文字可筛选）以及它思考的力度；从你的下一条消息起生效。在 `/model` 中，ctrl+f 把选中的模型设为备用模型（再按一次取消） |
| `/fast` | 当提供方在列表中提供时，开启模型的更快档位（ChatGPT 的“Fast”：回答更快，用量也更多）；再次输入 `/fast` 关闭。模型名旁会显示“· fast” |
| `/willington` | 开启或关闭 [Willington](WILLINGTON_INTEGRATION.md)。在支持的构建上请求机架绑定；片段 Follow Actions 和原生编辑还要求通过匹配实际库的自测。原生编辑仅支持 b5 macOS ARM64。启用前保持关闭；桥接包含 Willington 时可用。 |
| `/login`、`/logout` | 登录（在浏览器中或用代码登录 ChatGPT，或输入只显示为圆点的 API 密钥）或退出登录 |
| `/loop <what to reach>` | 按判定的轮次工作：一次修改，听修改前后，只有目标项变好、其他方面没变差才保留，然后下一个目标。所有项目达成、修改不再有帮助，或到 16 轮或 45 分钟时停止；按 Esc 或 `/loop stop` 也会停止。要匹配某个声音（“让我的 pad 听起来像 ~/ref.wav”）时改为旋钮搜索：单独输入 `/loop` 继续已暂停的搜索，`/loop stop` 结束它 |
| `/goal <what to reach>` | 跨多个回合坚持一个目标，每个回合后检查（测量过时看判定的数值），直到达成、受阻、停滞（连续三个回合没有更接近）或用完预算。`/goal` 显示它；`/goal resume`（用完的部分换上新的预算）、`/goal edit <words>`、`/goal new: <words>`（替换未完成的目标）、`/goal pause`（或 Esc）、`/goal clear`（没有未完成的目标时，结束暂停中的声音匹配搜索） |
| `/slots` | 显示每项聆听工作（分轨、扒谱、聆听、嵌入）由哪个模型负责，并用平常的话替换：`/slots listening gemini`、`/slots listening off`、`/slots listening <base URL>#<model>`、`/slots embeddings <.onnx 文件或 https 链接>`。新模型会先测试；`/slots back <job>` 撤回替换 |
| `/music` | 你自己的音乐，Kumi 在这台电脑上从中学习你的口味：文件夹、已听过的歌曲和进度。`/music add <folder>`、`/music remove <folder>`、`/music pause`、`/music resume`，以及 `/music forget`（经你确认后删除 Kumi 学到的内容）。Kumi 使用你的口味时，歌名和艺人名会发送给模型 |
| `/map` | 以两张全屏地图显示你的声音和歌曲（Tab 切换）：声音按类型着色，歌曲按风格着色。方向键移到该方向最近的点，Shift 加方向键平移，+ 和 − 缩放，点击选中一个点，`/` 按类型、文件夹、音色包或流派标签筛选，Space 播放该点，鼠标一移到某个点上就会播放（最多两秒；`h` 开关此功能，Kumi 会记住）。拖出一个框、按住 Ctrl 点击、`x`、`a`（筛选保留的全部）或 `g`（各点最近的点）可以选取多个点，Enter 把它们（或当前选中的点）放进你的调色板（palette）；Esc 会先放开已选取的点。地图会旋转，让长边横跨屏幕。大型素材库缩小时，越密集的地方越亮；放大后点会变大（典型的声音稍大一些），点聚集的地方会微微发光，并显示名称。在盲文点阵中，缩放和移动会平滑过渡。能显示图像的终端（kitty、Ghostty）用图像，其他终端用盲文点阵，基础控制台则显示列表。Esc 关闭 |
| `/map open` | 在浏览器中显示地图：指针移到点上显示名称，滚轮缩放，按住 Shift 拖动（或用 Lasso）选取多个点，筛选保留含有所输入文字的点，可显示或隐藏分组，点击播放一个点。它的操作会把文字放进输入框，Add to palette 把选取的点放进调色板。只从这台电脑提供，链接带随机密钥；通过 SSH 使用时，Kumi 会告诉你要转发哪个端口。`/map close` 关闭它，退出 Kumi 时也会关闭 |
| `/palette` | 你的调色板：在 `/map` 或浏览器地图中收集、准备使用的声音。`/palette` 列出它们；`/palette new <名称>`、`use <名称>`、`rename <名称>`、`forget <名称>`。调色板在会话之间保留，每个会话使用其中一个。Kumi 每次请求都会读取它：`/palette strict` 只使用其中的声音，`/palette loose [k]` 让每个声音带上素材库中与它最接近的 k 个（默认 5 个，你自己的项目用过的优先）。这些会在 `/map` 上淡淡标出，Kumi 用到其中一个时会说明，并问你是否把它加入调色板。右侧面板的 PALETTE 列出其中的声音：Space 播放，`x` 移除，最后一行请 Kumi 把它们作为套件载入 |
| `/memory` | Kumi 记住的一切：关于你和本工程的笔记、从你的工程中学到的东西、技巧、配方和经验；选择一项即可让它忘掉（笔记还可以修改文字或置顶，配方可以运行或忘掉） |
| `/note <id> <new words>` | 不经模型修改一条笔记的文字；在 `/memory` 中选择笔记的 “Change the words” 会替你开头 |
| `/recipes` | 你的配方：运行或忘掉一个。有空位的配方会写好一行 `/recipe`，填上已固定的对象，由你补完 |
| `/recipe <name> blank=value …` | 立即运行一个配方，不调用模型；含空格的值用引号括起，不带引号的数字或 true/false 按原值发送（要作为文字发送就加引号），没填的空位 Kumi 会指出 |
| `/jobs` | 后台任务：按 Enter 打开一个，在里面按 `x` 停止，按 `r` 重新运行被中断的任务。Kumi 回答时也能用 |
| `/status` | Kumi 连接到了什么、当前模型、学习素材库的进度、搜索中听过并记住的设置有多少（分布在多少个设备上），以及从记住的设置开始的搜索中它在被试的那一轮胜出的次数、使用 API 密钥时本次会话用掉的 token，以及当前对话和它的 ID |
| `/voice` | 说话功能：开始或停止、“停止后直接发送”、你说的语言和麦克风 |
| `/update` | 获取最新的 Kumi：它会先询问，然后关闭、更新，再以同一段对话重新打开 |
| `/changelog` | Kumi 的新功能：自你更新前的版本以来的全部条目（没有则显示最近三个版本），以及完整历史所在的位置 |
| `/help` | 按键和命令，分节显示在可滚动的面板中；在空输入框中按 `?` 也能打开 |
| `/quit` | 关闭 Kumi |

`/new`、`/reconnect`、`/refresh`、`/undo` 和 `/takeback` 只在 Kumi 没有在回答时有效；在回答过程中使用时，Kumi 会说明这一点，并保持原状。

## 即时短语

如果一条消息正是下面的短语之一，Kumi 不调用模型，立即修改 Live。回答会说明改了什么并标上 "instant"，这项修改出现在 HISTORY 中，可以撤销。目前只支持英文。

| 短语 | 作用 |
| --- | --- |
| `tempo 124`、`bpm 124`、`124 bpm` | 设置速度 |
| `mute drums`、`unmute drums` | 静音或取消静音一条轨道 |
| `solo bass`、`unsolo bass` | 独奏或取消独奏一条轨道 |
| `arm vocals`、`disarm vocals` | 让一条轨道准备录音，或取消 |
| `loop 9-16`、`loop bars 9 to 16`、`loop off` | 循环第 9 到 16 小节（含两端），或关闭循环 |
| `metronome on`、`metronome off` | 打开或关闭节拍器 |

- 轨道可以用名称指定（不区分大小写）；如果只有一条轨道的名称以你输入的内容开头，就是那条；也可以用位置指定：`mute track 3`。
- 带有更多内容的消息（"mute the drums a bit"、"solo the bass?"），或者 Kumi 无法确定是哪条轨道时，会按原样发给模型。
- 只有在 Kumi 没有回答时才会立即执行；回答过程中按 Enter，仍然像平时一样用于引导回答。
- **在 REAPER 中：** 带有速度标记或拍号标记的工程，`tempo` 和 `loop` 会交给模型（速度指的是哪个标记、一小节有多长，都取决于位置）。`loop 9-16` 还会打开循环播放。
- **在 Renoise 中：** `loop 9-16` 循环包含这些小节的序列槽位；在模式中间开始或结束的小节会交给模型。`arm` 会被拒绝：Renoise 在编辑模式开启时，把音符录入所选轨道的模式。速度范围为 32 到 999 BPM。
- 在 `~/.kumi/settings.json` 中设置 `"instantPhrases": false` 即可关闭。

## 按键

**输入与发送**

| 按键 | 操作 |
| --- | --- |
| Enter | 发送。Kumi 工作时，它会在当前步骤之后读到这条消息 |
| Tab（Kumi 工作时） | 改为在回答结束后发送这条消息；它会在输入框上方等待 |
| Alt-↑ | 把最后一条等待中的消息取回输入框 |
| Ctrl-J、Alt-Enter、Shift-Enter | 换行（Shift-Enter 仅在能报告它的终端中有效） |
| ↑ 和 ↓ | 在输入框的各行间移动，然后在之前发送过的内容中移动（跨 `/new` 和重启保留，不含机密信息） |
| Ctrl-A / Ctrl-E、Home / End | 行首 / 行尾 |
| Alt-← / Alt-→、Ctrl-← / Ctrl-→、Alt-B / Alt-F | 左移 / 右移一个词 |
| Ctrl-W、Alt-Backspace、Ctrl-Backspace | 剪切光标前的词 |
| Ctrl-K / Ctrl-U | 剪切到行尾 / 行首 |
| Ctrl-Y | 放回最后剪切的内容（连续剪切的，比如按两次 Ctrl-W，会一起放回）；紧接着按 Alt-Y 换成再前一次剪切的内容 |
| Ctrl-- (Ctrl-_) | 在输入框中撤销：一段连续的输入或删除、一次粘贴、一次剪切或一次口述的内容，每次撤销一项。在 Ctrl-- 会缩小终端显示的环境中（Windows Terminal、GNOME Terminal、Konsole），请改用 Ctrl-_（即 Ctrl-Shift--） |
| Ctrl-R | 按你输入的词在发送过的消息中往回查找：再按 Ctrl-R 找更早的，Enter 放进输入框，Esc 回到你的草稿 |
| Ctrl-G | 在 VISUAL 或 EDITOR 指定的你自己的编辑器中打开草稿（在窗口中打开的编辑器需要等待窗口关闭：`code --wait`）。保存的内容会回到输入框，Ctrl-- 可撤销 |
| Ctrl-S | 把草稿连同附件放到一边；输入框为空时按 Ctrl-S 取回（输入框有文字时两者互换） |
| Ctrl-T | 用说话代替打字：再按一次停止，或按住说话。你说的话会出现在光标处；Enter 立即停止并发送，Esc 放弃 |
| Ctrl-V | 把剪贴板中的图片（比如截图）添加到下一条消息；拖进窗口的文件也会这样添加 |
| 长段粘贴 | 超过 10 行或 1,000 个字符的粘贴会成为一个整体，例如 `[pasted 41 lines]`：Backspace 会整个删除它，Kumi 收到全部内容，对话中显示这个块和第一行的开头。Kumi 工作时暂存的消息、用 Alt-↑ 取回的、停止后放回的，以及用 ↑ 或 Ctrl-R 找回的，都保持为块。一条消息最多 256 KB：更长的会在输入框下方提示，按 Enter 它仍留在框中。历史文件只保留长消息的前 4,096 个字符，以 `…` 结尾；Kumi 启动后发送的消息用 ↑ 能完整找回 |
| 输入框为空时按 Backspace | 撤回最后添加的文件 |
| 输入框为空时按 `?` | 与 `/help` 一样打开按键和命令；在其他地方则输入问号。要让消息以问号开头，先输入一个空格，或直接粘贴 |

**停止与移动**

| 按键 | 操作 |
| --- | --- |
| Esc | 关闭 `/` 菜单；否则停止 Kumi 的回答（已完成的步骤保留，等待中的消息回到输入框）；否则取消固定的设备 |
| Esc Esc | Kumi 空闲、输入框为空且没有固定的设备时：按从新到旧列出你的请求，以及每个请求做了几处更改。选择一个，就像 `/takeback` 一样收回它以及之后的所有请求；在此之前，Kumi 会列出它无法撤销的更改，这些更改保持原样 |
| Ctrl-C | 停止 Kumi 的回答；空闲时清空输入框（按 ↑ 或 Ctrl-- 可找回文字）；输入框为空时退出 |
| Ctrl-B | 把回答中不用 Live 的耗时步骤（视频、网页、手册、你的素材库）作为任务移到后台，回答继续进行。在 tmux 中按两次 Ctrl-B |
| Ctrl-D | 空闲且输入框为空时退出 |
| Page Up / Page Down、鼠标滚轮 | 滚动对话（在标签页上滚动滚轮则滚动标签页）；有新文字到来时位置保持不动。向上滚动时，最上面一行保留你正在阅读的回答所对应的请求（点击即回到那里），最下面一行显示下方新到了几条 |
| Ctrl-Home / Ctrl-End | 对话开头 / 回到最新处 |
| Ctrl-O | 对话详情：每个请求、每个步骤收到和返回的内容（与完整导出相同，步骤最多显示 12 行）、每个回答（本次运行中给出的回答还会显示给出它的模型、开始时间和花费），以及进行中的这一轮。在其中，`/` 搜索（Enter 保留搜索，Esc 取消），`n` 和 `N` 在匹配之间移动，`{` 和 `}` 在请求之间跳转，`[` 把它打印到终端的回滚记录中直到按下任意键，`v` 在你的编辑器（`VISUAL` 或 `EDITOR`）中打开，`q`、Esc 或 Ctrl-O 关闭 |
| Ctrl-L | 重绘屏幕 |

**Live 窗格**

| 按键 | 操作 |
| --- | --- |
| Ctrl-\\ | 不足 100 列时打开或关闭抽屉；全宽时收起右侧窗格（对话占满整个窗口，输入框下方的一行会说明如何放回）或把它放回来；不足 24 行时，对两行条带也是如此 |
| Tab（空闲时） | 进入 FOCUS 的设备树，定位到 Live 的当前选择：↑↓ 移动，Enter 指向该行，Esc 或 Tab 回到输入 |
| Shift-Tab | 进入标签页（再按一次切到下一个标签页）：↑↓ 和 Page Up/Down 移动。在修改上，Enter、→ 或空格展开它的图示，Enter 或 ← 收起，展开时按 `u` 撤销；在记住的内容上，Enter 执行 forget；在 JOBS 的行上，Enter 打开任务，按 `x` 停止。Esc 或 Tab 返回 |
| 鼠标 | 点击行末的 **undo** 或 **forget**、修改的文字（展开图示）或任务的文字（打开任务）、标签页名称，或点击设备树中的设备以指向它；点击图钉取消固定。拖过文本，松开时即复制；双击复制一个词，三击复制一行（Kumi 折行的行会完整复制）。拖动期间显示保持不动，提示处会短暂显示“Copied”。此时按 Ctrl-C 会再复制一次，而不会停止 Kumi 或退出；Kumi 空闲时按 Esc 只会让这一行消失。在 settings.json 中设置 `"copyOnSelect": false` 后，选中的文字会保留，Ctrl-C 或右键复制，Esc 取消选择。按住 Shift 拖动使用终端自己的选择功能（iTerm2 中为 Option），用于 Kumi 的复制到不了剪贴板时（例如通过 SSH 且终端不允许时） |

**面板**（`/model`、`/effort`、`/login`、`/memory` 等）：↑↓ 移动，Enter 选择，Esc 关闭；在 `/model`、`/memory`、`/resume` 和 `/recipes` 中，输入文字可筛选列表。在 `/model` 中，ctrl+f 设定备用模型。在 `/resume` 中，Tab 在 This Set 和 All Sets 之间切换，ctrl+f 可重命名、置顶、删除所选对话或复制它的 ID。对话属于其他工程时，Kumi 会询问：在 Live 中打开它的工程（推荐）、在这里作为副本继续，或取消（Esc）。**Your answer**（以带编号选项的问题结尾的回答）：按编号再按 Enter 即可回答。Kumi 推荐的选项带有标记，并一开始就被选中；没有人在时（运行过 `/goal`、`/loop` 或匹配，或 Kumi 的窗口已在其他窗口后面 2 分钟），倒计时结束后 Kumi 会选它（“Taking 2 (recommended) in 60 s · any key cancels”）。在密钥输入框中，粘贴密钥后按 Enter；Kumi 会向提供方验证它，无法连接提供方时则不经验证直接保存。在 ChatGPT 登录面板中，`c` 复制链接（或代码），`d` 改用代码登录而不用浏览器，`b` 改回浏览器。在 `/btw` 面板中，↑↓、Page Up/Down 和鼠标滚轮滚动，←→ 翻看之前的回答，`c` 复制，Esc、Enter 或 Space 关闭面板。在任务详情中，↑↓ 和滚轮滚动，`x` 停止任务，`r` 重新运行，`c` 复制报告，Esc 关闭。在帮助中，↑↓、Page Up/Down、Home、End 和滚轮滚动，Esc、Enter 或 `?` 关闭它；输入或粘贴文字也会关闭它，输入的内容会进入输入框。设置 `"mouse": "off"` 时，滚轮不会滚动其中任何一个。

**自定义按键。** `~/.kumi/keys.json` 可以给 Kumi 的动作换上别的按键（`KUMI_KEYS_FILE` 可指定文件的其他位置）：

```json
{ "composer.undo": ["ctrl+z"], "app.background": ["ctrl+b", "f5"], "pane.toggle": [] }
```

- 每个动作的列表会取代它原来的按键，空列表则让它不再有按键。按键写作 `ctrl`、`alt`（或 `option`）、`shift` 加上名称：`enter`、`tab`、`esc`、`backspace`、`delete`、`insert`、`up` 或 `↑`（其他方向键同理）、`home`、`end`、`pageup`、`pagedown`、`space`、`f1` 到 `f12`，或单个字符（`ctrl+-`、`ctrl++`）。用空格隔开的两个键（`esc esc`）表示先后按下：只有当输入框里没有别的动作接收第一个键时，它才开始这一组；第二个键要在半秒内按下；这样的组合只在输入框里有效。
- 多个动作共用一个键时，先试 keys.json 里的，再按一贯的顺序试 Kumi 自己的。
- 在输入文字的地方（输入框、列表、`/` 菜单），单个字符不能作为按键，在那里它会被当作文字输入。输入框为空时，`?` 总是打开帮助；也可以给 `app.help` 再加按键（`f1`）。
- 启动时，Kumi 会把用不上的内容说一次：不认识的动作、读不懂的按键、输入文字处的单个字符、keys.json 给了同一个键的两个动作，以及排在 Kumi 自身按键之前的键。
- 帮助（`/help`、`?`）按 keys.json 显示按键，但输入框下方、面板、地图和设置里的提示仍写着 Kumi 自己的按键。

| 位置 | 动作与 Kumi 自己的按键 |
| --- | --- |
| 输入框 | `composer.send`（enter）、`composer.send-after`（Kumi 工作时的 tab）、`composer.take-back-waiting`（alt+↑）、`composer.new-line`（ctrl+j、alt+enter、shift+enter）、`composer.up`、`composer.down`、`composer.left`、`composer.right`、`composer.word-left`（alt+←、ctrl+←、alt+b）、`composer.word-right`（alt+→、ctrl+→、alt+f）、`composer.line-start`（home、ctrl+a）、`composer.line-end`（end、ctrl+e）、`composer.backspace`、`composer.take-back-file`（输入框为空时的 backspace）、`composer.delete`（delete、ctrl+d）、`composer.cut-word`（ctrl+w、alt+backspace、ctrl+backspace）、`composer.cut-to-end`（ctrl+k）、`composer.cut-to-start`（ctrl+u）、`composer.yank`（ctrl+y）、`composer.yank-earlier`（alt+y）、`composer.undo`（ctrl+-、ctrl+_）、`composer.history-search`（ctrl+r）、`composer.editor`（ctrl+g）、`composer.stash`（ctrl+s） |
| Kumi | `app.stop`（esc）、`app.take-back`（输入框为空时的 esc esc）、`app.interrupt`（ctrl+c）、`app.quit`（ctrl+d）、`app.background`（ctrl+b）、`app.paste-picture`（ctrl+v）、`app.redraw`（ctrl+l）、`app.help`（没有自己的按键：`?`）、`voice.talk`（ctrl+t）、`conversation.page-up`、`conversation.page-down`、`conversation.start`（ctrl+home）、`conversation.latest`（ctrl+end）、`conversation.details`（ctrl+o） |
| 窗格 | `pane.toggle`（ctrl+\\）、`pane.tabs`（shift+tab）、`pane.tree`（tab）；标签页里有 `tabs.up`、`tabs.down`、`tabs.page-up`、`tabs.page-down`、`tabs.open`（enter）、`tabs.next`（shift+tab）、`tabs.leave`（esc、tab）；设备树里有 `tree.up`、`tree.down`、`tree.pin`（enter）、`tree.leave`（esc、tab） |
| `/` 菜单 | `menu.up`、`menu.down`、`menu.complete`（tab）、`menu.run`（enter）、`menu.close`（esc） |
| 回看（ctrl+r） | `search.back`（ctrl+r）、`search.erase`（backspace）、`search.take`（enter、tab）、`search.cancel`（esc、ctrl+c） |
| 聆听时 | `voice.send`（enter）、`voice.drop`（esc、ctrl+c） |
| 选中的文字 | `selection.copy`（ctrl+c）、`selection.clear`（esc） |
| 帮助 | `help.up`、`help.down`、`help.page-up`、`help.page-down`、`help.top`（home）、`help.end`（end）、`help.close`（esc、enter、ctrl+c、`?`） |
| 旁问回答（`/btw`） | `aside.up`、`aside.down`、`aside.page-up`、`aside.page-down`、`aside.earlier`（←）、`aside.later`（→）、`aside.copy`（c）、`aside.close`（esc、enter、space、ctrl+c） |
| 任务详情 | `job.up`、`job.down`、`job.page-up`、`job.page-down`、`job.stop`（x）、`job.again`（r）、`job.copy`（c）、`job.close`（esc、enter、space、ctrl+c） |
| 列表 | `list.up`、`list.down`（↓、tab）、`list.next-tab`（有标签页的列表里的 tab）、`list.erase`（backspace）、`list.choose`（enter）、`list.choose-other`（ctrl+f）、`list.close`（esc、ctrl+c） |
| 密钥输入框 | `key.check`（enter）、`key.erase`（backspace）、`key.clear`（ctrl+u）、`key.cancel`（esc、ctrl+c） |
| ChatGPT 登录 | `chatgpt.copy`（c）、`chatgpt.by-code`（d）、`chatgpt.by-browser`（b）、`chatgpt.cancel`（esc、ctrl+c） |
| 对话详情（ctrl+o） | `details.up`、`details.down`、`details.page-up`、`details.page-down`、`details.top`（home）、`details.end`（end）、`details.search`（/）、`details.next-match`（n）、`details.previous-match`（N）、`details.next-request`（}）、`details.previous-request`（{）、`details.print`（[）、`details.edit`（v）、`details.close`（esc、q、ctrl+c、ctrl+o）；输入搜索时有 `details.search-erase`（backspace）、`details.search-keep`（enter）、`details.search-drop`（esc） |
| 地图（`/map`） | `map.up`、`map.down`、`map.left`、`map.right`（方向键）、`map.pan-up`、`map.pan-down`、`map.pan-left`、`map.pan-right`（shift 加方向键）、`map.zoom-in`（`+`、`=`）、`map.zoom-out`（`-`、`_`）、`map.play`（space）、`map.hover`（h）、`map.pick`（x）、`map.pick-all`（a）、`map.grow`（g）、`map.gather`（enter）、`map.filter`（`/`）、`map.switch`（tab）、`map.let-go`（esc）、`map.close`（esc、q、ctrl+c）；输入筛选时，`map.filter-erase`（backspace）、`map.filter-keep`（enter）、`map.filter-drop`（esc） |
| 设置 | `setup.up`、`setup.down`（↓、tab）、`setup.choose`（enter）、`setup.later`（esc）、`setup.quit`（ctrl+c、ctrl+d） |

## 纯文本模式

设置 `KUMI_UI=plain`，或者输入或输出经过管道时，Kumi 会改用逐行显示的纯文本界面，适合屏幕阅读器和日志。它支持 `/help`、`/status`、`/undo`、`/stop`、`/refresh`、`/reconnect`、`/new [name]`、`/resume [编号、ID 或名称]`（以及 `/conversations`）、`/rename <name>`、`/fork [name]`、`/export [md|html|json]`、`/share`（都会保存文件并告诉你位置）、`/daw [name]`、`/model [provider/model]`、`/effort [level|default]`、`/fast`、`/logout <provider>`、`/memory`、`/forget <id>`、`/note <id> <new words>`、`/pin <id>`、`/unpin <id>`、`/recipes`、`/willington`、`/update`、`/changelog` 和 `/quit`，但没有 `/btw`、`/goal`、`/loop`、`/slots`、`/music`、`/map`、`/jobs`、`/takeback` 或 `/copy`。`/resume` 先为本工程的对话编号，再为其他工程的对话编号；Kumi 关于其他工程对话的提问用 1、2 或 3 回答（等待回答时命令照常可用，输入其他内容会再问一次）。后台任务开始和结束时会告诉你，你的请求所需要的任务会在 Kumi 空闲时接着进行。请在 shell 中用 `kumi login` 登录。Ctrl-C 停止回答，空闲时则退出。

## 离开 Kumi 时

- **终端中的状态：** Kumi 用 OSC 7501 报告它在做什么（工作中，附 NOW 的文字和运行进度；等你选择或登录；完成；失败），支持它的终端会显示在标签页或侧栏中。Windows Terminal、ConEmu 和 Ghostty 还会收到 OSC 9;4 进度。Kumi 退出时两者都会清除。
- **Kumi 的窗口在其他窗口后面时**，Kumi 会在以下情况告诉你：完成一个用时 15 秒或更久的回答、需要你选择或登录、等待套餐上限、因错误停止，或结束一次 `/goal`、`/loop` 或匹配：
  - 桌面通知：用终端自己的通知（Ghostty、WezTerm 和 foot 用 OSC 777；iTerm2 用 OSC 9；kitty 用 OSC 99），否则用 macOS 的通知（Terminal.app 等）、Linux 上的 `notify-send`，或 Windows Terminal 的响铃（任务栏会闪烁）；
  - Live（或 Renoise）状态栏中的一行（“Kumi · done · the bass is darker”）；从不弹出对话框，也不进入 HISTORY。REAPER 没有 Kumi 能写入的状态栏。
  - 每 10 秒最多一条（等待中的最新一条届时送出）；Live 录音时一条也不送，等录音停止；你回来后不再发送。
  - 对从不告知自己是否在前台的终端，在 Mac 上，只有在有事要告诉你时，Kumi 才询问系统哪个应用在前台。
  - 没有看到通知？请在“系统设置 › 通知”中允许你的终端（若是 macOS 自身的通知，则为“脚本编辑器”）发送通知。
- **电脑保持唤醒**：在 Kumi 工作期间（一个回答、`/goal`、等待套餐上限）；显示器可以休眠。
- **设置**（`~/.kumi/settings.json`）：`"notifications": "off"`、`"liveStatusBar": false`、`"keepAwake": false`，以及 `"answerAfter"`（Kumi 选择推荐答案之前的秒数；`0` 表示从不）。

## 终端

Kumi 会检测终端能显示多少种颜色；`KUMI_COLOR`（`truecolor`、`256`、`16`、`none`）可覆盖检测结果，并且会遵循 `NO_COLOR`。在终端符号可能无法显示的地方（旧版 Windows 控制台、Linux 控制台），Kumi 用双字母徽标代替图标；可用 `KUMI_ICONS=glyphs` 或 `badges` 自行选择。修改的图示在 kitty 和 Ghostty 中（24 位色、不在 tmux 内）显示为图像，在其他终端中用盲文点字和半格方块绘出，在符号可能无法显示的地方则以文字呈现；`KUMI_PICTURES=cells` 可让图示始终用字符单元绘出。Kumi 启动时会询问终端是什么终端（据此在 SSH 下也能显示图像、通知和可点击的链接）；`KUMI_PROBE=off` 可让它不询问。在 Windows 上推荐使用 Windows Terminal。Kumi 回答中的网页和邮件链接在 iTerm2、Ghostty、WezTerm、kitty、Windows Terminal 和 VS Code 的终端中（不在 tmux 内）可以点击打开（有的终端需按住 Cmd 或 Ctrl）；在这些终端里，除非文字看起来像另一个地址，地址不再跟在文字后面，在其他终端中仍会显示。拖动只复制文字；`/copy` 和 `/export` 会保留地址。在不支持 kitty 键盘协议的终端中，请用 Ctrl-J 或 Alt-Enter 换行。在 tmux 中，`set -g allow-passthrough on` 让 Kumi 的状态和通知能够透传，焦点报告需要 `set -g focus-events on`；通过 SSH 时，`set -g set-clipboard on` 让 Kumi 的复制能到达你的剪贴板。支持该协议的终端会告知按住的 Ctrl-T 何时松开；在其他终端中，按键的重复停止时 Kumi 就停止聆听。

退出时，Kumi 会在道别之前把对话打印到终端的回滚记录中：你的请求、Kumi 的回答，每一串步骤一行，并像 `/export` 一样去掉机密（很长的对话只保留最后 200 KB）。在 settings.json 中设置 `"transcriptOnQuit": false` 可关闭此功能。`"mouse": "off"` 把鼠标交给终端（或 tmux）：终端自己的选择和复制可用，Page Up 和 Page Down 滚动，滚轮不会因此发送 ↑ 或 ↓。此时 Kumi 的点击、拖动复制以及地图中的鼠标操作都不可用。

## 设计说明

供开发这个应用的人参考（`crates/kumi/src/tui/`）。

**原则。** Kumi 掌管整个窗口并绘制每一个单元格，因此各窗格各自滚动、位置保持不动。不用边框：各区域以背景深浅区分。颜色总是有含义的。文字优先于符号，步骤读起来是音乐上的事（“looked at Bass and Drums”），而不是工具名称。动画只用来表示有事正在发生或正在变化。窗格只是视图：运行时以数据描述要绘制的内容，因此其他前端也能绘制同样的东西。

**调色板**（`style.rs`）。灰色从 `#0e0f12`（底色）到 `#f4f6f8`（最亮），一种强调色，即薄荷绿 `#86e3b5`，以及 Live 自己的轨道颜色（很暗的颜色会调亮后显示）。警告 `#e7b45f`，错误 `#ee8479`，节拍灯 `#ffe14d`。Kumi 记住的内容每类各有一种颜色：笔记 `#8cc8ff`，技巧 `#c7a6ff`，配方 `#f2a6c4`，经验 `#e7c88f`。

**基础**，全部由终端原语构建，不用任何 UI 框架：

1. 终端 I/O（`tty.rs`、`probe.rs`）：原始模式、备用屏幕、括号粘贴（bracketed paste）、SGR 鼠标、焦点事件，以及终端提供时的 kitty 键盘协议；退出、崩溃和收到信号时都会恢复终端。界面启动时，Kumi 会向终端询问一次它是什么终端以及它的颜色，最多等待四分之一秒。即使在 SSH 下环境变量里没有终端名称，也会根据这些回答选择图片、通知和颜色深度。
2. 输入（`keys.rs`）：带修饰键的按键（xterm 和 CSI u）、粘贴、鼠标、被拆分到多次读取中的序列，以及通过短暂超时判定的单独 Esc。在 kitty 协议下，按住的键的重复和松开都是各自的事件。
3. 屏幕与渲染器（`screen.rs`、`render.rs`）：一个单元格网格；每一帧都与上一帧比较差异，只把发生变化的单元格写出，且在同步更新中进行。颜色从 24 位依次回退到 256 色、16 色和无色。
4. 文本（`width.rs`、`wrap.rs`）：字素宽度（宽 CJK 字符和 emoji 占两个单元格）、换行与截断。
5. 帧（`scheduler.rs`）：重绘会被合并，动画时钟只在有东西运动时运行。

`app.rs` 绘制布局并处理输入；`editor.rs` 是输入框，`transcript.rs` 是对话，`tree.rs` 是设备树，`tabs.rs` 是标签页。测试通过一个小型终端解释器重放渲染器的输出，该解释器必须精确重现预期的帧。

**尚未完成：** NOW 中针对定位器和新轨道的图示；路由的汇入（fan-in）或侧链示意图；超长片段的概览条；跨多条轨道的修改在 HISTORY 中显示为一个可展开的条目；提议一并撤销依赖于某项修改的其他修改；把条目标记为“已在 Live 中撤销”；通过 macOS 辅助功能实现 FOCUS（精确的控件、片段标签页、Browser 项目）。
