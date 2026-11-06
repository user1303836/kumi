# Kumi for Hermes Agent

[English](../en/HERMES.md) · 简体中文 · [日本語](../ja/HERMES.md)

Kumi 也是 [Hermes Agent](https://hermes-agent.nousresearch.com) 的插件。智能体仍然是 Hermes（终端、Hermes Desktop、网页仪表盘或聊天应用），Kumi 为它接上你的 DAW。每次修改前 Hermes 都会先征求你的同意（除非你关闭了它的确认），Kumi 把每一处修改记入 HISTORY，并能在运行期间撤销其中大部分。两者都在你的电脑上运行。

单独的 Kumi（在终端中使用的 `kumi` 应用）和 Kumi for Hermes 是两套独立的安装，只用其中一个也可以。

## 安装

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

`kumi` 是 Kumi 在 Hermes 插件目录中的名称。在 Kumi 列入目录之前，请从 Kumi 的代码仓库安装：`hermes plugins install user1303836/kumi#hermes-plugin`，然后同样启用它。

在 Hermes Desktop 中，还要在 **技能与工具 → 插件** 里打开 Kumi 的**桌面**开关：插件的桌面开关默认是关闭的。

## 设置

让 Hermes“为 Live 设置 Kumi”（REAPER 或 Renoise 也一样），或在终端运行 `hermes kumi setup --daw reaper`（`--daw` 可取 `ableton`、`reaper` 或 `renoise`；不加时，为 Kumi 设置中的 DAW 进行设置，未更改时是 Live）。设置分两步，另有可选的第三步，每一步之前 Hermes 都会征求你的同意：

1. **Kumi 的运行时：** 从 Kumi 的 GitHub 发布页下载适用于你电脑的 Kumi 官方版本（26–33 MB），用插件固定的 SHA-256 校验，再解压到 Hermes 为这个配置保存的数据中（82–121 MB）。
2. **Kumi 的 DAW 组件：** Live 的 Remote Script（含 Kumi Ears 设备）、REAPER 的扩展或 Renoise 的工具，安装到设置向你展示的确切位置。请先退出 Live：Live 运行时无法安装它的 Remote Script。REAPER 和 Renoise 可以保持打开，装好后重启即可（在 Windows 上更新 REAPER 扩展时，请先退出 REAPER）。
3. **可选的 ffmpeg：** 如果你的电脑上已有 ffmpeg，设置会询问是否让 Kumi 用它读取 MP3、FLAC 等压缩音频。无论是否允许，设置都会完成；不使用时，Kumi 只能读取 WAV 和 AIFF。

Hermes Desktop 会完整显示每一步要你确认的内容。终端里的对话或聊天应用可能只显示其中一部分，所以在那里 Kumi 绝不会只拿出一步的一部分来征求同意，而是请你在终端运行 `hermes kumi setup`，或改用 Hermes Desktop。

然后在 DAW 中：
- **Live：** 在设置 → Link, Tempo & MIDI 中把 **AbletonMcpBridge** 选为控制界面。
- **REAPER：** 重启 REAPER。
- **Renoise：** 启用 Kumi 工具。

你最先设置的 DAW 会在同一次确认中成为 Kumi 的 DAW（DAW 设置）。之后再设置另一个 DAW 时，Kumi 仍连接原来的 DAW：设置结束时会告诉你切换的步骤——在终端运行 `hermes kumi setup --daw <daw> --use`，它会在修改设置前征求你的同意。你也可以在 Kumi 的插件设置中更改 DAW（配置文件 config.yaml 中的 `plugins.entries.kumi.settings.daw`）。如果在你的环境中无法写入 Hermes 的设置，设置会说明这一点，以及应该在哪里设置 DAW。

如果这台电脑上的另一套 Kumi（单独的 Kumi，或另一个 Hermes 配置中的 Kumi for Hermes）已经安装了 Kumi 的 Live 组件，设置会告诉你，并建议直接使用那套安装而不是替换它。这样 Kumi for Hermes 会读取那套安装的连接文件，并把自己的连接状态写在那里，但绝不使用它的登录信息或设置。对于 REAPER 和 Renoise，Kumi for Hermes 目前还不能使用另一套 Kumi 的组件：设置会说明这一点，并保持原样。

设置不会启动或退出你的 DAW、替你选择控制界面，也不会保存你的工程。

## 可以让它做什么

### 像参考曲那样为混音做母带

“听听我的混音，按这首参考曲的样子做母带。”
- **聆听：** 在 Live、REAPER 和 Renoise 中，Kumi 会聆听你批准的那部分混音并测量；每次聆听都会先征求同意。在 Live 中，它通过设置安装的 Kumi Ears 设备（需要 Max for Live），在每条音轨的设备之后、推子之前聆听：Main 的设备（包括其限制器）会被听到，Main 的推子不会，所以 Kumi 用 Main 的设备去接近目标，从不动它的推子。
- **比较：** 把你的混音和参考文件放在一起测量：响度、真峰值、频谱平衡、立体声宽度和动态。很宽的立体声和反相的立体声也能准确测量，包括频谱平衡和波峰因数。单声道或无声的文件没有立体声宽度，过短的音频可能没有响度或响度范围：Kumi 会把它们显示为无法测量，而不是猜测。
- **参考的一部分，或多个参考：**“和参考曲从 1:00 开始的 30 秒比较”（最长 60 秒，且在文件范围内），或一次与最多五个参考比较。Kumi 以相同权重把它们测成一个目标及其离散范围，并说明每个测量值来自几个参考。某个参考给不出的值（例如超出其采样率的频段，或单声道文件的立体声宽度）不计入该值，并说明原因。离散范围包含最小容差，并不是置信度。没有单一的“更好”分数、没有平均后的音频，也没有真峰值目标；母带处理仍由 Kumi 提出、由你批准。Kumi 保留的片段从它自己的开头播放。
- **做母带：** 它会在总线（master）轨道上提出母带处理（EQ、粘合压缩、限制器），作为一次需要 Hermes 征求同意的修改。
- **重复：** 它会再聆听一次并比较，最多三轮。每一轮的修改都记入 HISTORY，可以撤销。

### 写一段节拍或贝斯线

“在新音轨上写一个四拍底鼓加反拍踩镲”，然后“在下面加一条 A 小调的贝斯线”。
- Kumi 把音符作为一次需要 Hermes 征求同意的修改写入。
- “播放”会开始播放，并先征求同意；“停止”会停止播放，从不询问。两者都不会改变工程中的任何内容，只会改变播放的起点。REAPER 从某个小节或拍开始播放，Renoise 从某个序列位置和行开始。在 Live 中，Kumi 会启动它写入的 Session 片段或某个场景，或从 Live 自己的开始标记播放 Arrangement。Kumi 绝不会开始一次会录音的播放。
- “做三个变奏：更密的、更稀的，以及一个摇摆感更强的。”每个版本在你批准前 Hermes 都会展示；每个版本都写成一个新片段（REAPER 中是 item，Renoise 中是 pattern），绝不覆盖原来的。最多 8 个版本。如果没有足够的空位放下每个版本，Kumi 会拒绝，而不是少做几个。要舍弃某个版本，撤销它即可。

### 找一个采样并用上

“帮我找一个带颗粒感、偏暗的军鼓。”
- 在你列出的采样文件夹中，按测得的声音特征和文件名、文件夹名中的词查找。在 Hermes Desktop 中会打开声音地图。
- 也可以按速度、调、长度或类型来找：“A 小调、120 BPM 的鼓 loop”“短于 300 ms 的偏暗底鼓”。每个结果都会说明它为什么匹配、依据是什么：Kumi 在该文件夹最近一次扫描时记录的速度、调或音高（来自音频本身，或来自文件名、文件夹名；重新扫描会更新），或者它测得的特征（“暗”“亮”“颗粒感”“lo-fi”“有冲击力”）。速度匹配同一速度或其半速、倍速，调匹配同一个调、它的关系大小调，或 one-shot 的根音；回答会说明是哪一种，而且不会移调。没有记录速度或调的声音，按这个条件找不到。艺人和流派的词只匹配名称和文件夹：Kumi 不会识别流派或艺人。“更多类似的”按在这张地图上你的声音中与那个声音的接近程度排序，绝不以百分比表示。在 Hermes Desktop 中，地图的列表会显示每个声音记录的数值，以及它匹配的原因。
- “把它用在军鼓上”：在 Live 中放进军鼓的 Simpler（包括 Drum Rack 打击垫上的），在 REAPER 中放到一条新音轨上，在 Renoise 中放进一个新乐器，作为一次需要 Hermes 征求同意的修改。“把它放到一条新音轨上”：在 Live 中放进新音轨上的一个新 Simpler。
- “把它放到打击垫 2 上”：在 Live 中放到那个 Drum Rack 打击垫上——放进该打击垫的 Simpler，或放到空打击垫上的一个新 Simpler 中，每次修改一个打击垫。Kumi 无法撤销加载到空打击垫上的操作，并会在你批准前说明：Live 的撤销通常分两步恢复，所以请立即一次按一下 Cmd+Z，打击垫一空就停下。Live 的重做不会把采样带回来：请让 Kumi 重新加载。Kumi 绝不会为了腾出位置而清空打击垫。在 Hermes Desktop 中，地图的 **Use on pad…** 会把这个请求放进对话。

### 日常操作

- “我现在在看什么？”Kumi 读取你选中的音轨、设备和片段。
- “这个 Serum 上的 Drive 是做什么的？”在 Live 中，Kumi 会读取它对该插件的指南，以及你为它配置（Configure）的参数；在任何 DAW 中，都能读取它的插件格式目录，以及它已验证过的控件。它只读取：不会扫描你已安装的插件，不会打开插件的编辑器或按下 Configure，也不会写入预设、调音或波表。Kumi 只是见过的数值范围，并不是它保证的范围。
- “把贝斯调低 3 dB。”Hermes 先准确说明会改变什么并征求同意，Kumi 再做修改并记入 HISTORY。
- “把贝斯的输出送到 Drum Bus。”Kumi 为音轨设置输出去向。在 Live 中，它不会更改音轨的输入、让音轨预备录音或更改其监听设置：这些是用于录音的。
- “在 keys 音轨的 Instrument Rack 中添加一条链。”在 Live 中，给机架添加链是只能用 Live 自己的撤销来恢复的修改（见[限制](#限制)）。
- “比较这两个导出的文件。”Kumi 测量两个文件：响度、真峰值、频谱。
- “停止扫描。”对采样文件夹的长时间扫描，你一说就会停止。你原来的声音地图会保留，除非扫描已经开始保存新的地图。
- “撤销刚才那个。”Kumi 撤销你在这个对话中最新的一次修改，包括其中每一步，并在一次列出这些步骤的确认中完成。“撤销军鼓那次修改”则在还能撤销时撤销那一条。
- “按这个教程做出贝斯：<YouTube 链接>。”Hermes 读取视频的文字稿；数值只出现在画面上时，它会在浏览器里查看那些时刻（或请你截图），然后 Kumi 用需要 Hermes 征求同意的修改把它做出来。Kumi 不会下载视频。

Kumi 也能与 ElevenLabs 和 Mirelo 的声音配合使用：见[集成](#集成可选)。

### 脚本和 Max for Live 设备

当 Kumi 已知的修改都不合适时，Hermes 可以为它编写代码。
- **脚本：**“在 Renoise 中写一个 Lua 脚本，把每隔一个的 pattern 反转。”Live 中用 Python，REAPER 和 Renoise 中用 Lua。批准时会显示 Hermes 所说的用途、开头几行、大小和 SHA-256，以及完整脚本保存的位置；Hermes Desktop 会显示全部代码。脚本以你的 DAW 对电脑的访问权限运行，Kumi 会在 10 秒后（Renoise 中为 0.8 秒）停止它。Kumi 无法撤销脚本：DAW 自己的撤销也许可以。HISTORY 会保留代码，用其中的 **Show code** 可以再次查看。
- **Max for Live 音频效果器：**“做一个带 drive 和 tone 的 Max for Live 软削波器，放在贝斯的 EQ 后面。”Kumi 用 GenExpr 代码写出一个新设备，放到 `User Library/Kumi/Audio Effects`，最多有 16 个自己的控件，另加 Mix 和 Output。它加入的声音保持在 −0.3 dBFS 以下；音轨本身的信号原样通过。你决定时，Hermes Desktop 会显示它的控件和代码。把它放进你的 Set 是另一次需要 Hermes 征求同意的修改：Kumi 把它加载到音轨链的末尾，并且可以撤销这一步。再把它从那里移走是第三次修改，只能用 Live 自己的撤销来恢复。撤销之后，这个文件仍留在你的用户库中。
- **对话读取过外部内容后，每次都会询问：** 网页、文字稿、Hermes 从互联网获取的文件、其他插件的结果，或运行过的任何终端命令或代码。已在你磁盘上的文件、已安装的技能，以及你的音色库和工程中的名称不算在内。在这样的对话中，每个脚本和设备都会带着代码征求同意，Hermes 的确认关闭时一个也不会运行。在没有读取过外部内容的对话中，在本次会话中允许一个脚本，就对这个对话中的脚本都有效；每个新设备仍会征求同意。
- **在询问任何人之前就会拒绝：** 超过 400 行或 32 KB 的脚本或设备代码；含有隐藏字符或改变文字方向字符的脚本，或会运行进程、访问网络的脚本（`os.execute`、`io.popen`、`subprocess`、`os.system`、`socket`、`urllib`、`ctypes`）。这项检查是一道保护措施，不是沙盒。

Hermes 会在每次修改、撤销、聆听和播放前征求同意，因为它的确认功能是开启的。在本次会话中或始终允许某个 Kumi 调用，只对这个对话中的同类调用（修改、撤销、聆听、播放、采样文件夹的添加与扫描）有效，也包括恢复该对话的一次性运行（`hermes chat -q --resume`）；另一个对话、新的一次性运行或定时任务，会重新询问或被拒绝。Kumi 只在你自己的回合中行动：Hermes 在后台处理对话（每轮之后的回顾）或回答旁支问题（`/btw`）时，无论你允许了什么，Kumi 都只读取。只能用 DAW 自己的撤销来恢复的修改，会作为单独的一类征求同意，所以允许其他修改并不包括它。设置步骤的批准只针对它自己的计划。选择**始终允许**会在 Hermes 的 `command_allowlist`（你的配置中的 config.yaml）里留下一条记录，Kumi 从不编辑它：要取消，请删除其中以 `plugin_rule:kumi:` 开头的条目。

使用 `--yolo`、对话中的 `/yolo` 或 `approvals.mode: off` 时，Hermes 会不经询问直接执行 Kumi 的修改、撤销、聆听、播放、采样文件夹的添加与扫描、设置步骤，以及设置了 Mirelo 时它的付费生成和上传（以你的点数上限为限）。脚本和设备在那里也会不经询问运行，但只限于没有读取过外部内容的对话：读取过的对话中，它们会被拒绝。`/kumi` 会说明这一点。如果 `approvals.single_query_mode` 或 `delegation.subagent_auto_approve` 让修改不经询问就通过，`hermes kumi status` 也会说明。一次性的 `hermes -z` 也等同于 yolo：Kumi 的调用会不经询问直接执行，而且 Kumi 无法把它与普通对话区分开。

## 东西在哪里

- **所有界面：** Kumi 的工具、技能（`kumi:daw`、`kumi:sounds`、`kumi:listening`，以及用于 Mirelo 的 `kumi:generate`）以及查看状态的 `/kumi`。在消息中写 `@kumi:focus`、`@kumi:history` 或 `@kumi:status`，即可附上 Kumi 最后看到的内容。
- **Hermes Desktop：** 状态标签、显示 FOCUS 和 HISTORY 的侧边栏、单独标签页中的声音地图、测量结果卡片和命令面板中的命令。脚本或设备等待你批准时，侧边栏会显示它的全部代码（以及设备的控件）；脚本在 HISTORY 中的那一条有 **Show code**。按钮只会把请求放进对话，不会自己修改你的工程。
- **网页仪表盘：** 一个包含 HISTORY、状态和地图的 Kumi 标签页。

## 设置项

| 设置 | 默认 | 作用 |
|---|---|---|
| DAW | Live | Kumi 连接哪个 DAW。你最先设置的 DAW 会成为它；用 `hermes kumi setup --daw <daw> --use` 切换（`ableton`、`reaper` 或 `renoise`） |
| 采样文件夹 | 无 | 为声音地图允许 Kumi 读取的文件夹：扫描只读取其中的音频文件，其他内容一概不扫描。不能是整个磁盘或卷、你的主文件夹或它上面的文件夹，也不能在 Hermes 自己的文件夹（`HERMES_HOME`）之内或之上；可以用 `~/`。链接和联接点绝不会把 Kumi 带出你列出的文件夹；在 Windows 上，Kumi 会跳过云端占位文件（例如 OneDrive 的） |
| 告诉智能体我在看什么 | 开 | 当你查看的内容变化时，在连接 Kumi 的对话中加入一段简短说明：你正在看什么（FOCUS），以及 Kumi 不在时工程里有哪些变化 |
| 从聊天应用运行脚本 | 关 | 允许聊天应用（Discord、Slack、Telegram 等）中的对话请求在你的 DAW 中运行脚本，或构建 Max for Live 设备。每一个都会带着代码征求同意，Hermes 的确认关闭时一个也不会运行 |
| 用模型解读测量结果 | 关 | 把测量数值和一张小的频段能量图也发给你在 Hermes 中为 Kumi 聆听任务设置的模型（未更改时为 Vision 的模型），用文字解读；该模型必须能接受图片 |

## 哪些内容会离开你的电脑

- **设置：** 每个 Kumi 版本从 Kumi 的 GitHub 发布页下载一次运行时。
- **对话：** Kumi 记下的工程信息（FOCUS、变化）和测量结果，会作为对话的一部分发给你在 Hermes 中使用的模型。Kumi 的状态告诉模型设置进行到哪一步时，不会提到你电脑上的任何文件夹。
- **用模型解读测量结果**（只有打开后才会）：把测量数值和一张小的频段能量图发给你在 Hermes 中为 Kumi 聆听任务设置的模型，或 Hermes 为该任务配置的后备服务商，绝不发送音频本身或文件路径。重试和后备可能增加时间和 token 费用。
- **音频留在本地：** Kumi 在你的电脑上测量音频，不会把音频发给任何模型。在聊天应用（Telegram、Discord 等）中，如果你让 Hermes 把声音发给你，它会作为附件通过该应用送达。
- **聆听音轨或混音：**
  - **批准和时间范围：** 它会先征求同意，只聆听你批准的时间范围，最长 60 秒（在 Live 中最多为 Arrangement 中的 256 拍），并且 DAW 必须处于停止状态。Kumi 不会替你停止播放。
  - **你的工程：** 渲染临时使用的走带、路由和设备设置之后都会恢复。聆听后 REAPER 可能显示工程已修改，因为只有保存才能清除这个标记；Kumi 从不保存。
  - **Renoise 的速度：** 渲染可能改变它的 BPM 和每拍行数。只有它们仍是渲染留下的值时，Kumi 才会改回去；如果已经不同，它会两者都保持原样，并说明无法恢复速度。
  - **音频：** DAW 渲染到 Kumi 在该 DAW 中的组件的聆听文件夹；Kumi 把这一个文件复制到自己的数据中，在那里测量。
  - **Live：** Kumi 在 Live 停止且未录音时，通过 Kumi Ears 采集这段时间范围，结束后移除该设备；之后详细视图会重新显示你之前打开的片段。Live 可能显示 Set 已修改，并且 Live 自己的撤销历史中会留下这次聆听的临时设备步骤：在 Live 中按撤销可能会让 Kumi Ears 回来。这些步骤不是 Kumi 的 HISTORY 中的修改。
- **脚本**以你的 DAW 对电脑的访问权限运行：它读取、写入或发送什么，取决于你批准的代码。Kumi 会把每个已批准脚本的代码保存在自己的数据中。
- **Max for Live 设备**是 Kumi 写入 Live 用户库（`User Library/Kumi/Audio Effects`）的新文件，位于 Kumi 自己的数据之外。在你删除之前，它们会一直留在那里。
- **Live Set 的副本：** 当 Live 中的一次修改进行到第三步，或包含可能丢失素材的步骤（删除、清除一段范围，或把片段移到已有内容的位置）时，Kumi 会把最后保存的 Set 复制一份放在它旁边（`<Set>.backup-<日期>.als`），每个保存的版本只复制一次。未保存的 Set 不会被复制，Kumi 也从不保存 Set 本身。副本会一直保留，直到你删除它。
- **Mirelo**（只有设置后才会）：见[集成](#集成可选)。
- **定时任务**（Hermes 的 cron）只读取 Kumi 最后发布的状态和 HISTORY。它的回答（修改标题、音轨名称）只会通过该任务的投递目标离开你的电脑；Kumi 的任务模板默认投递到本机。
- Kumi for Hermes 绝不使用单独的 Kumi 的登录、模型服务商或更新程序，也不发送遥测数据。

## 限制

- 你的 DAW 必须运行在运行 Hermes 的那台电脑上。连接到远程 Hermes 的 Hermes Desktop 控制的是那台电脑上的 DAW，而不是你本机的。
- Live 支持 macOS 和 Windows；REAPER 和 Renoise 支持 macOS、Windows 和 Linux。
- Kumi 会聆听你批准的时间范围，最长 60 秒（在 Live 中最多为 Arrangement 中的 256 拍）。在所有 DAW 中，它都能测量音频文件、音色库中的声音和 Kumi 自己的结果。
- Kumi 只读取打开的工程、你列出的采样文件夹中的音频文件、它对你批准的时间范围的渲染和采集，以及其他 Hermes 插件保存在 Hermes 生成音频文件夹（`<HERMES_HOME>/cache/generated/audio`，Kumi 从不创建这个文件夹）中的已完成音频，用于放置或测量。
- Kumi 能直接读取 WAV 和 AIFF。对于 MP3、FLAC 等压缩音频，如果你在设置时允许，它会使用电脑上已有的 ffmpeg（绝不下载）。每次启动前，Kumi 都会确认它仍是你允许的那个 ffmpeg 文件（不检查它加载的库，这些库可能随升级而变化）。ffmpeg 只会拿到 Kumi 正在读取的那一个文件：绝不是 URL 或播放列表，也不从网络读取任何内容。Kumi 可读取的文件最大 50 MiB，最多测量其中 12 分钟。
- Kumi 的 REAPER 扩展只在连接该 DAW 的单独 Kumi 请求时，才会在已保存的工程中保存一个 ID（Renoise 工具则保存在歌曲中）；Kumi for Hermes 从不请求。一旦保存，这个 ID 会随工程一起保存。
- 在 Windows 上，网络文件夹（`\\server\share`，或通过链接访问的共享文件夹）中的采样无法在 Hermes Desktop 或仪表盘中播放：它们只播放这台电脑本地磁盘上的文件。
- 在 Hermes Desktop 或 Hermes 的终端界面中停止，或在聊天应用中发送 `/stop` 或 `/new`，会取消正在进行的 Kumi 修改、撤销或聆听。之后 Kumi 无法确定它是否已完成，所以再次请求前请先查看 HISTORY。在经典的 `hermes chat` CLI 中，停止不会传到 Kumi：调用会一直运行到完成或达到时间上限。
- Kumi 只能在做出修改的那个 Kumi 运行时仍在运行时撤销该修改。运行时会在对话结束、Hermes 卸载插件或退出，或 15 分钟没有任何操作（为某个对话连接着 DAW 时为 2 小时）后停止。它正在运行任务时（例如音色库扫描），最多会继续运行 6 小时。停止之后修改仍留在 HISTORY 中，但要撤销请使用 DAW 自己的撤销。
- 在 Live 中，有几种修改只能用 Live 自己的撤销来恢复：给机架添加链、移动设备，以及把采样加载到打击垫上。Kumi 会在你批准前说明这一点，以及如何用 Cmd+Z（Windows 上为 Ctrl+Z）恢复，HISTORY 也会标记这一条。请立即撤销：如果你之后在 Live 中做过任何操作，Cmd+Z 会先撤销那些操作。这样的修改不包含其他内容：其他步骤 Kumi 会另外征求同意。脚本在所有 DAW 中都一样：Kumi 无法撤销它，DAW 自己的撤销也许可以。
- 在 Live 中，如果 Kumi 修改了某条音轨的调音台设置、速度或某个设备的参数，而你之后又手动改过这个值，Kumi 不会撤销那次修改：撤销会说明这一点，并且不做任何改动。撤销在征求同意前，会列出每个值的当前值，以及撤销后会恢复成的值。在 REAPER 和 Renoise 中，即使你之后手动改过这个值，撤销也会把它改回去。
- 在 REAPER 和 Renoise 中，Kumi 连接期间你手动做的编辑，在你问“有什么变化”时不会被列出；Kumi 下一次查看时仍会看到工程当前的样子。
- Kumi for Hermes 不会操作 Live 的菜单、录音，也不会保存或打开工程。在 Live 中它不会为音轨编组：请选中它们并按 Cmd+G（Windows 上为 Ctrl+G）。

## 集成（可选）

Kumi 的核心功能不需要其中任何一个。

### 其他 Hermes 插件的声音（ElevenLabs）

“用 ElevenLabs 做一个上升音效，放到 FX 音轨上。”其他 Hermes 插件生成声音后，Kumi 可以测量它，或把它放进你的工程。Kumi 只读取 Hermes 生成音频文件夹（`<HERMES_HOME>/cache/generated/audio`，Kumi 从不创建这个文件夹）中的已完成音频，并先把文件复制到自己的数据中；放置是一次需要 Hermes 征求同意的修改。

### Mirelo（付费）

“用 Mirelo 做一段黑胶噪声”或“把这段贝斯转成 MIDI。”有 [Mirelo](https://mirelo.ai) 账户时，Kumi 可以请 Mirelo 根据提示词生成音效，或把音频文件转成 MIDI 音符。

- **打开之前不会使用：** 在 Kumi 的设置中添加你的 **Mirelo API key**（作为 Hermes 机密保存，`MIRELO_API_KEY`），打开 **Use Mirelo for paid generation**（使用 Mirelo 付费生成）和你需要的类别（**Mirelo sound effects**（音效）、**Mirelo audio to MIDI**（音频转 MIDI）），并把 **Mirelo credit cap**（点数上限）设为大于 0（默认的 0 会拒绝所有付费生成）。只有密钥时，Kumi 绝不会使用 Mirelo。
- **是点数，不是钱：** 每次生成都会先带着 Mirelo 以 API 点数计的估价征求同意，这次批准只对这个估价有效，且只用一次。为生成 MIDI 而发送音频文件会单独征求同意；在本次会话中允许，也只对那个对话中的那个文件有效。上限按 Kumi 的整个使用期间计算：每次生成最多预留其估价的点数；扣费在 Mirelo 告知之前可能一直未知（期间预留仍计入上限）；扣费超过估价时，新的付费生成会暂停。在定时任务、`hermes chat -q`、Webhook 或 Hermes 的 API 服务器中，不会付费，也不会上传。
- **查看和处理点数（在终端中）：** `hermes kumi credits` 显示已用、预留和剩余的点数，以及尚未结清的预留。`hermes kumi credits unfreeze` 让付费生成重新开始；`hermes kumi credits release <id>` 在你到 Mirelo 的控制台核实后，解除扣费一直未知的预留（如果 Mirelo 之后又告知了这笔扣费，仍会计入，并暂停新的付费生成）。每个命令都会先征求你的同意，而且只能由你执行：Hermes 和它的模型无法运行它们。
- **离开你电脑的内容：** 你的提示词，以及转 MIDI 时你批准的音频文件，会连同你的密钥发送到 `api.mirelo.ai`，以及 Mirelo 的 API 为每个任务给出的上传和下载链接。这些链接的主机由 Mirelo 选择（它的存储或分发网络），并不是一份固定的清单。Mirelo 能否用你发送的内容训练，取决于你的 Mirelo 账户设置；Kumi 自己没有这方面的设置。
- **生成结果**保存在 Kumi 的数据中。放置它是 Hermes 会另外征求同意的修改，MIDI 音符的时间以秒计。要转成 MIDI 的音频文件需在 Mirelo 的限制之内（30 分钟、50 MiB）；MP3 等压缩音频需要设置时允许的 ffmpeg。关闭某一类会停止该类的新生成（已经在进行的生成，只要密钥还在，仍会取回结果）；删除密钥后，Kumi 不再联系 Mirelo。

### 放进工程的声音

两者的声音放进工程后，都会留在这个配置的 `plugin-data/kumi/` 文件夹中，工程从那里播放它。删除这个文件夹之前，请把这些工程连同文件一起保存（Live：文件菜单中的 **Collect All and Save**；REAPER：**Save project as…** 并勾选 **Copy all media into project directory**），否则这些采样会丢失。

## 遇到问题时

- `hermes kumi doctor`（在对话中用 `/kumi doctor`）会检查 Kumi 在这个 Hermes 配置中需要的一切：Hermes 的批准设置、设置、连接、打开的项目和声音地图。对每个问题，它都会告诉你该怎么做。它不会更改任何内容。
- `/kumi`（在终端中用 `hermes kumi status`）会告诉你设置还缺什么，以及 Kumi 能否读取压缩音频。当 `plugins.isolation` 为 `host` 时，Hermes 不会加载 `hermes kumi` 这类插件命令，请改在 Hermes Desktop 的对话中设置。
- Hermes Desktop 中看不到 Kumi：检查 技能与工具 → 插件 里 Kumi 的 **Agent** 和**桌面**开关是否打开，以及 Hermes 中是否已启用（`hermes plugins enable kumi`）。
- 网页仪表盘中没有 Kumi 标签页：如果安装 Kumi 时仪表盘已经在运行，请重启它（`hermes dashboard`）。
- Live 连不上：检查是否已把 AbletonMcpBridge 选为控制界面。
- REAPER 没有响应（某个修改显示 “unsure”，或 Kumi 的状态说可能有对话框开着）：REAPER 有一个窗口在等你处理，比如评估版提醒或音频设备的询问。在 REAPER 中关掉它，然后再让 Hermes 试一次。
- 聆听被拒绝，因为 Kumi 在你的 DAW 中的组件版本较旧：重新运行设置以更新它，然后重启 DAW。
- 在 Live 中聆听时提示 Kumi Ears 缺失或不可用：它需要 Max for Live。请重新运行设置；聆听从不安装它。
- 在 Live 中聆听时提示“Live is not following the Arrangement”：在 Live 中按下 Back to Arrangement，然后再试一次。
- 聆听要求你停止播放，或 Renoise 提示“is not ready to render”：停止播放和录音，关闭 Renoise 的 Edit Mode，并等它的速度扫描完成。Kumi 不会替你停止播放或录音。
- “播放”在 DAW 正在录音或录音开启时被拒绝：Kumi 绝不会开始一次会录音的播放。请先在 DAW 中关闭录音。
