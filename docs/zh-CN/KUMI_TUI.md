# Kumi 的命令、按键与界面

[English](../en/KUMI_TUI.md) · 简体中文 · [日本語](../ja/KUMI_TUI.md)

Kumi 终端应用的参考：屏幕上显示什么、每条命令和按键，以及纯文本模式。[指南](KUMI_GUIDE.md)介绍 Kumi 能做什么。

## 界面

Kumi 占据整个终端窗口，并在 Kumi 关闭、崩溃或被停止时把窗口恢复原样。

- **顶栏：** Kumi、工程名称、模型及其推理强度，以及与 Live 的连接状态。Live 播放时，速度旁边有一盏黄灯随节拍闪烁（每小节第一拍最亮）。
- **对话**（左侧）：你的消息、Kumi 的回答及其步骤，用平常的话描述（“looked at your Set”），每条都附有耗时。正在进行的步骤按类型显示动画（搜索、阅读、构建设备、修改、聆听、播放）；同一步骤做了多次会折叠成一行（“read a page ×3”）。Kumi 听到的内容显示为一个小频谱，它记住的内容单独占一行：✎ 笔记，◆ 技巧，↻ 配方，✦ 从匹配中得到的经验。
- **FOCUS**（右上）：你在 Live 中所处的位置，随你的选择变化而更新：轨道的设备树（含机架链）、以小型钢琴卷帘显示的片段音符，或 Session 视图、编曲视图的条带。你指向的设备（点击它，或在 Live 中使用 **Ask Kumi about this**）会被固定，用于你接下来的消息。
- **NOW**（右侧中部）：Kumi 正在做的事，随进行实时绘出：某个值修改前后的样子、新片段的音符、色块、落入设备链的设备、“▶ Playing from the start marker”。
- **标签页**（右下）：**HISTORY** 先列出 Kumi 最近记住的三项内容（各带 **forget**），然后是每项修改，最新的在前，各带 **undo**；无法撤销时则显示 **kept** / **no undo** / **check Live**。**GOAL** 显示 `/goal`（设定之前显示“No goal yet”）：目标、最佳得分及其趋势、领先的候选方案，以及用时。
- **输入框**（左下）：等待发送的消息显示在它上方，被固定的设备显示为一个小标签，随下一条消息发送的文件则显示名称、类型、大小和 ×。为空时显示 “ctrl+t to talk”。Kumi 聆听时，它的底行会显示一个闪烁的薄荷绿 `●`、时间和电平表，右侧是可用的按键；不用红色，因为在 Live 中红色表示录音。

欢迎界面会显示自你上次使用以来工程中发生的变化；有更新的 Kumi 时也会显示；第一次时还会说明 Kumi 正在后台学习你的素材库。

**设置**会在还有步骤未完成时最先出现：登录、Live 中 Kumi 的桥接（缺失，或比 Kumi 自带的旧），然后是 Control Surface。它列出这三步，已完成的以薄荷绿显示并注明所选内容，并询问当前这一步需要什么；Kumi 等待时（等浏览器、安装或 Live），字标下方的薄荷绿波形会动起来。↑↓ 和 Enter 用于选择，Esc 把这一步留到以后，之后的会话在同一窗口中显示。已经设置好的 Kumi 会直接进入会话。

宽度不足 100 列时，Live 窗格会折叠为输入框上方的两行条带（先是你所在的位置和 Kumi 正在做的事，然后是最近一次修改及其撤销）。窗口小于 24×8 时，Kumi 会请你把窗口调大。

## 终端中的命令

以 `kumi <command>` 运行；在仓库副本中，以 `npm run kumi -- <command>` 运行。

| 命令 | 作用 |
| --- | --- |
| `kumi` | 在 Live 当前打开的工程上打开 Kumi |
| `kumi --inference-only` | 不连接 Live，只聊天 |
| `kumi --bridge-config <absolute path>` | 使用你自己的桥接配置 |
| `kumi login` | 登录：询问使用 ChatGPT 还是 API 密钥 |
| `kumi login <provider>` | 登录 `openai-codex`（ChatGPT；没有浏览器时加 `--device`），或用 API 密钥登录 `anthropic`、`openai`、`opencode` 或 `opencode-go` |
| `kumi logout <provider>` | 移除 Kumi 在该提供方的登录 |
| `kumi model [<provider>/<model>]` | 显示或选择模型，也可以是你电脑上的模型（`ollama/<model>`） |
| `kumi auth` | 哪些提供方可用、当前模型以及登录文件（从不显示机密信息） |
| `kumi bridge [--yes] [--allow-dirty]` | 在 Live 关闭时：把桥接装入 Live，或将其更新到最新。`--yes` 确认 Live 已关闭；`--allow-dirty` 允许带有未提交修改的检出副本安装其桥接 |
| `kumi doctor` | 检查 Node、登录、你电脑上的模型服务器、桥接、Live、扩展、素材库、视频程序、说话功能、Live 的菜单和终端；告诉你该运行什么 |
| `kumi library [--rebuild]` | Kumi 学习你的声音、预设和工程进行到了哪里；`--rebuild` 全部重新学习 |
| `kumi update [--check \| --rollback]` | 获取最新的 Kumi（桥接较旧时也一并更新）；`--check` 只检查；`--rollback` 回到上一个版本（Live 已关闭时连同其桥接；仅限已安装的 Kumi） |
| `kumi report` | 写出 `~/kumi-report-<date and time>.txt`，出问题时把它发给我们 |
| `kumi uninstall [--all] [--yes]` | 卸载已安装的 Kumi；`--all` 同时删除对话、笔记、配方和登录信息；`--yes` 跳过第一个确认问题 |
| `kumi --version`（或 `-v`）、`kumi --help` | 版本；帮助 |

## Kumi 内的命令

输入 `/` 打开这些命令的菜单：↑↓ 选择，Tab 补全，Enter 运行，Esc 关闭。以路径开头的消息（拖进终端的文件）是一条消息，而不是命令。

| 命令 | 作用 |
| --- | --- |
| `/new` | 开始新的对话。之前的内容仍留在屏幕上，以一条分隔线隔开；上一段对话会被保存，HISTORY 中的撤销依然有效 |
| `/btw <question>` | 随时顺便问个问题，即使 Kumi 正在工作也可以：根据目前为止的对话回答，不使用工具，显示在一个面板中，且不加入对话。只输入 `/btw` 会再次显示上一个回答 |
| `/conversations` | 本工程保存的对话（最近 20 段）；选择一段即可继续 |
| `/reconnect` | 通过新的桥接重新连接 Live，保留对话（之前的修改将失去 Kumi 的撤销） |
| `/undo` | 撤销 Kumi 最近一次修改 |
| `/stop` | 停止 Live：片段、走带和录音。同时停止 Kumi 的回答 |
| `/refresh` | 重新读取工程，不询问模型 |
| `/copy` | 把 Kumi 的上一个回答复制到剪贴板 |
| `/model`、`/effort` | 选择模型（来自各提供方自己的列表，然后是你电脑上的模型服务器：Ollama、LM Studio 以及 settings.json 中写的服务器；输入文字可筛选）以及它思考的力度；从你的下一条消息起生效 |
| `/fast` | 当提供方在列表中提供时，开启模型的更快档位（ChatGPT 的“Fast”：回答更快，用量也更多）；再次输入 `/fast` 关闭。模型名旁会显示“· fast” |
| `/willington` | 开启或关闭 [Willington](WILLINGTON_INTEGRATION.md) 的绑定（映射机架宏、为宏和变体命名、机架链区域，以及自检通过时的 Follow Actions）；在你开启之前一直关闭，Kumi 会在启动时告诉你。只有 Live 中的桥接带有 Willington 时才可用 |
| `/login`、`/logout` | 登录（在浏览器中用 ChatGPT 登录，或输入只显示为圆点的 API 密钥）或退出登录 |
| `/goal <what to reach>` | 追求一种声音，直到 Kumi 做到为止。只输入 `/goal` 会继续已暂停的目标；`/goal stop`（或 `/goal end`）结束它 |
| `/memory` | Kumi 记住的一切：关于你和本工程的笔记、从你的工程中学到的东西、技巧、配方和经验；选择一项即可让它忘掉（笔记还可以修改文字或置顶，配方可以运行或忘掉） |
| `/note <id> <new words>` | 不经模型修改一条笔记的文字；在 `/memory` 中选择笔记的 “Change the words” 会替你开头 |
| `/recipes` | 你的配方：运行或忘掉一个。有空位的配方会写好一行 `/recipe`，填上已固定的对象，由你补完 |
| `/recipe <name> blank=value …` | 立即运行一个配方，不调用模型；含空格的值用引号括起，不带引号的数字或 true/false 按原值发送（要作为文字发送就加引号），没填的空位 Kumi 会指出 |
| `/status` | Kumi 连接到了什么、当前模型、学习素材库的进度，以及使用 API 密钥时本次会话用掉的 token |
| `/voice` | 说话功能：开始或停止、“停止后直接发送”、你说的语言和麦克风 |
| `/update` | 获取最新的 Kumi：它会先询问，然后关闭、更新，再以同一段对话重新打开 |
| `/help` | 按键和命令，以一条说明的形式显示在对话中 |
| `/quit` | 关闭 Kumi |

`/new`、`/reconnect`、`/refresh` 和 `/undo` 只在 Kumi 没有在回答时有效；在回答过程中使用时，Kumi 会说明这一点，并保持原状。

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
| Ctrl-W、Alt-Backspace、Ctrl-Backspace | 删除光标前的词 |
| Ctrl-K / Ctrl-U | 删除到行尾 / 行首 |
| Ctrl-T | 用说话代替打字：再按一次停止，或按住说话。你说的话会出现在光标处；Enter 立即停止并发送，Esc 放弃 |
| Ctrl-V | 把剪贴板中的图片（比如截图）添加到下一条消息；拖进窗口的文件也会这样添加 |
| 输入框为空时按 Backspace | 撤回最后添加的文件 |

**停止与移动**

| 按键 | 操作 |
| --- | --- |
| Esc | 关闭 `/` 菜单；否则停止 Kumi 的回答（已完成的步骤保留，等待中的消息回到输入框）；否则取消固定的设备 |
| Ctrl-C | 停止 Kumi 的回答；空闲时清空输入框；输入框为空时退出 |
| Ctrl-D | 空闲且输入框为空时退出 |
| Page Up / Page Down、鼠标滚轮 | 滚动对话（在标签页上滚动滚轮则滚动标签页）；有新文字到来时位置保持不动 |
| Ctrl-Home / Ctrl-End | 对话开头 / 回到最新处 |
| Ctrl-L | 重绘屏幕 |

**Live 窗格**

| 按键 | 操作 |
| --- | --- |
| Tab（空闲时） | 进入 FOCUS 的设备树，定位到 Live 的当前选择：↑↓ 移动，Enter 指向该行，Esc 或 Tab 回到输入 |
| Shift-Tab | 进入标签页（再按一次切到下一个标签页）：↑↓ 和 Page Up/Down 移动，Enter 执行该行的 **undo** 或 **forget**，Esc 或 Tab 返回 |
| 鼠标 | 点击行末的 **undo** 或 **forget**、标签页名称，或点击设备树中的设备以指向它；点击图钉取消固定。拖动时按住 Shift（iTerm2 中为 Option）可选择文字 |

**面板**（`/model`、`/effort`、`/login`、`/memory` 等）：↑↓ 移动，Enter 选择，Esc 关闭；在 `/model`、`/memory`、`/conversations` 和 `/recipes` 中，输入文字可筛选列表。在密钥输入框中，粘贴密钥后按 Enter；Kumi 会向提供方验证它，无法连接提供方时则不经验证直接保存。在 ChatGPT 登录面板中，`c` 复制链接。在 `/btw` 面板中，↑↓ 和 Page Up/Down 滚动，←→ 翻看之前的回答，`c` 复制，Esc、Enter 或 Space 关闭面板。

## 纯文本模式

设置 `KUMI_UI=plain`，或者输入或输出经过管道时，Kumi 会改用逐行显示的纯文本界面，适合屏幕阅读器和日志。它支持 `/help`、`/status`、`/undo`、`/stop`、`/refresh`、`/reconnect`、`/new`、`/conversations [n]`、`/model [provider/model]`、`/effort [level|default]`、`/fast`、`/logout <provider>`、`/memory`、`/forget <id>`、`/note <id> <new words>`、`/pin <id>`、`/unpin <id>`、`/recipes`、`/willington`、`/update` 和 `/quit`，但没有 `/btw`、`/goal` 或 `/copy`。请在 shell 中用 `kumi login` 登录。Ctrl-C 停止回答，空闲时则退出。

## 终端

Kumi 会检测终端能显示多少种颜色；`KUMI_COLOR`（`truecolor`、`256`、`16`、`none`）可覆盖检测结果，并且会遵循 `NO_COLOR`。在终端符号可能无法显示的地方（旧版 Windows 控制台、Linux 控制台），Kumi 用双字母徽标代替图标；可用 `KUMI_ICONS=glyphs` 或 `badges` 自行选择。在 Windows 上推荐使用 Windows Terminal。在不支持 kitty 键盘协议的终端中，请用 Ctrl-J 或 Alt-Enter 换行。支持该协议的终端会告知按住的 Ctrl-T 何时松开；在其他终端中，按键的重复停止时 Kumi 就停止聆听。

## 设计说明

供开发这个应用的人参考（`crates/kumi/src/tui/`）。

**原则。** Kumi 掌管整个窗口并绘制每一个单元格，因此各窗格各自滚动、位置保持不动。不用边框：各区域以背景深浅区分。颜色总是有含义的。文字优先于符号，步骤读起来是音乐上的事（“looked at Bass and Drums”），而不是工具名称。动画只用来表示有事正在发生或正在变化。窗格只是视图：运行时以数据描述要绘制的内容，因此其他前端也能绘制同样的东西。

**调色板**（`style.rs`）。灰色从 `#0e0f12`（底色）到 `#f4f6f8`（最亮），一种强调色，即薄荷绿 `#86e3b5`，以及 Live 自己的轨道颜色（很暗的颜色会调亮后显示）。警告 `#e7b45f`，错误 `#ee8479`，节拍灯 `#ffe14d`。Kumi 记住的内容每类各有一种颜色：笔记 `#8cc8ff`，技巧 `#c7a6ff`，配方 `#f2a6c4`，经验 `#e7c88f`。

**基础**，全部由终端原语构建，不用任何 UI 框架：

1. 终端 I/O（`tty.rs`）：原始模式、备用屏幕、括号粘贴（bracketed paste）、SGR 鼠标、焦点事件，以及终端提供时的 kitty 键盘协议；退出、崩溃和收到信号时都会恢复终端。
2. 输入（`keys.rs`）：带修饰键的按键（xterm 和 CSI u）、粘贴、鼠标、被拆分到多次读取中的序列，以及通过短暂超时判定的单独 Esc。在 kitty 协议下，按住的键的重复和松开都是各自的事件。
3. 屏幕与渲染器（`screen.rs`、`render.rs`）：一个单元格网格；每一帧都与上一帧比较差异，只把发生变化的单元格写出，且在同步更新中进行。颜色从 24 位依次回退到 256 色、16 色和无色。
4. 文本（`width.rs`、`wrap.rs`）：字素宽度（宽 CJK 字符和 emoji 占两个单元格）、换行与截断。
5. 帧（`scheduler.rs`）：重绘会被合并，动画时钟只在有东西运动时运行。

`app.rs` 绘制布局并处理输入；`editor.rs` 是输入框，`transcript.rs` 是对话，`tree.rs` 是设备树，`tabs.rs` 是标签页。测试通过一个小型终端解释器重放渲染器的输出，该解释器必须精确重现预期的帧。

**尚未完成：** NOW 中针对定位器和新轨道的图示；路由的汇入（fan-in）或侧链示意图；超长片段的概览条；跨多条轨道的修改在 HISTORY 中显示为一个可展开的条目；提议一并撤销依赖于某项修改的其他修改；把条目标记为“已在 Live 中撤销”；通过 macOS 辅助功能实现 FOCUS（精确的控件、片段标签页、Browser 项目）。
