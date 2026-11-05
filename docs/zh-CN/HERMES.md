# Kumi for Hermes Agent

[English](../en/HERMES.md) · 简体中文 · [日本語](../ja/HERMES.md)

Kumi 也是 [Hermes Agent](https://hermes-agent.nousresearch.com) 的插件。智能体仍然是 Hermes（终端、Hermes Desktop、网页仪表盘或聊天应用），Kumi 为它接上你的 DAW。每次修改前 Hermes 都会先征求你的同意，Kumi 把每一处修改记入 HISTORY 并附带撤销。两者都在你的电脑上运行。

单独的 Kumi（在终端中使用的 `kumi` 应用）和 Kumi for Hermes 是两套独立的安装，只用其中一个也可以。

## 安装

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

在 Hermes Desktop 中，还要在 **技能与工具 → 插件** 里打开 Kumi 的**桌面**开关：插件的桌面开关默认是关闭的。

## 设置

让 Hermes“为 Live 设置 Kumi”（REAPER 或 Renoise 也一样），或在终端运行 `hermes kumi setup`。设置分两步，另有可选的第三步，每一步之前 Hermes 都会征求你的同意：

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

- “我现在在看什么？”Kumi 读取你选中的音轨、设备和片段。
- “帮我找一个更暗的军鼓。”Kumi 在你列出的采样文件夹中查找；在 Hermes Desktop 中会打开声音地图。
- “停止扫描。”对采样文件夹的长时间扫描，你一说就会停止。你原来的声音地图会保留，除非扫描已经开始保存新的地图。
- “把它放到第 2 个打击垫上。”Hermes 先准确说明会改变什么并征求同意，Kumi 再做修改并记入 HISTORY。
- “真的更暗了吗？”Kumi 测量两个文件：响度、真峰值、频谱。
- “撤销军鼓那次修改。”只要还能撤销，Kumi 就撤销那一条。

Hermes 会在每次修改前征求同意，因为它的确认功能是开启的。Kumi 的批准只适用于一个对话，也包括恢复该对话的一次性运行（`hermes chat -q --resume`）；定时任务、新的一次性运行或另一个对话，总是会重新询问或被拒绝。选择**始终允许**会在 Hermes 的 `command_allowlist`（你的配置中的 config.yaml）里留下一条记录，Kumi 从不编辑它：要取消，请删除其中以 `plugin_rule:kumi:` 开头的条目。

使用 `--yolo`、对话中的 `/yolo` 或 `approvals.mode: off` 时，Hermes 会不经询问直接执行 Kumi 的修改、撤销、音轨聆听、采样文件夹的添加与扫描，以及设置步骤；`/kumi` 会说明这一点。如果 `approvals.single_query_mode` 或 `delegation.subagent_auto_approve` 让修改不经询问就通过，`hermes kumi status` 也会说明。一次性的 `hermes -z` 也等同于 yolo：Kumi 的修改会不经询问直接执行，而且 Kumi 无法把它与普通对话区分开。

## 东西在哪里

- **所有界面：** Kumi 的工具、技能（`kumi:daw`、`kumi:sounds`、`kumi:listening`）以及查看状态的 `/kumi`。在消息中写 `@kumi:focus`、`@kumi:history` 或 `@kumi:status`，即可附上 Kumi 最后看到的内容。
- **Hermes Desktop：** 状态标签、显示 FOCUS 和 HISTORY 的侧边栏、单独标签页中的声音地图、测量结果卡片和命令面板中的命令。按钮只会把请求放进对话，不会自己修改你的工程。
- **网页仪表盘：** 一个包含 HISTORY、状态和地图的 Kumi 标签页。

## 设置项

| 设置 | 默认 | 作用 |
|---|---|---|
| DAW | Live | Kumi 连接哪个 DAW。你最先设置的 DAW 会成为它；用 `hermes kumi setup --daw <daw> --use` 切换 |
| 采样文件夹 | 无 | 为声音地图允许 Kumi 读取的文件夹，其他地方一概不扫描。不能是整个磁盘或卷、你的主文件夹或它上面的文件夹，也不能在 Hermes 自己的文件夹（`HERMES_HOME`）之内或之上；可以用 `~/`。链接和联接点绝不会把 Kumi 带出你列出的文件夹；在 Windows 上，Kumi 会跳过云端占位文件（例如 OneDrive 的） |
| 告诉智能体我在看什么 | 开 | 当你查看的内容变化时，在连接 Kumi 的对话中加入一段简短的 FOCUS 和变化摘要 |
| 用模型解读测量结果 | 关 | 把测量数值和一张小的频段能量图也发给你的 Hermes 模型，用文字解读 |

## 哪些内容会离开你的电脑

- **设置：** 每个 Kumi 版本从 Kumi 的 GitHub 发布页下载一次运行时。
- **对话：** Kumi 记下的工程信息（FOCUS、变化）和测量结果，会作为对话的一部分发给你在 Hermes 中使用的模型。Kumi 的状态告诉模型设置进行到哪一步时，不会提到你电脑上的任何文件夹。
- **用模型解读测量结果**（只有打开后才会）：把测量数值和一张小的频段能量图发给你在 Hermes 中为 Kumi 聆听任务设置的模型，或 Hermes 为该任务配置的后备服务商，绝不发送音频本身或文件路径。重试和后备可能增加时间和 token 费用。
- **音频留在本地：** Kumi 在你的电脑上测量音频，不会把音频发给任何模型。在聊天应用（Telegram、Discord 等）中，如果你让 Hermes 把声音发给你，它会作为附件通过该应用送达。
- **在 REAPER 和 Renoise 中聆听音轨或混音**之前会先征求同意，只播放你批准的节拍范围（最多 60 秒），而且只在 DAW 停止时进行。路由和走带可能会短暂改变，结束后会恢复。DAW 会把它渲染到拥有 Kumi 的该 DAW 组件的那套 Kumi 安装的 listening 文件夹中，Kumi 只复制那一个文件，然后删除它。这需要较新的 Kumi 的 DAW 组件：Kumi 提示时，请再运行一次设置并重启 DAW。Renoise 只聆听在歌曲内、起止都落在整行 pattern 上的范围，其他范围会直接拒绝，而不会多播放你未批准的内容。在 REAPER 中聆听音轨后，工程可能显示为已修改（只有保存才能清除这个标记，而 Kumi 从不保存）。
- **定时任务**（Hermes 的 cron）只读取 Kumi 最后发布的状态和 HISTORY。它的回答（修改标题、音轨名称）只会通过该任务的投递目标离开你的电脑；Kumi 的任务模板默认投递到本机。
- Kumi for Hermes 绝不使用单独的 Kumi 的登录、模型服务商或更新程序，也不发送遥测数据。

## 限制

- 你的 DAW 必须运行在运行 Hermes 的那台电脑上。连接到远程 Hermes 的 Hermes Desktop 控制的是那台电脑上的 DAW，而不是你本机的。
- Live 支持 macOS 和 Windows；REAPER 和 Renoise 支持 macOS、Windows 和 Linux。
- 在 Live 中，Kumi for Hermes 目前还不会聆听音轨或混音：它会说明这一点，并且不播放任何内容。在任何 DAW 中，它都能测量音频文件、音色库中的声音和 Kumi 自己的结果。
- Kumi 只读取打开的工程、你列出的采样文件夹，以及其他 Hermes 插件保存在 Hermes 生成音频文件夹（`<HERMES_HOME>/cache/generated/audio`，Kumi 从不创建这个文件夹）中的已完成音频，用于放置或测量。
- Kumi 能直接读取 WAV 和 AIFF。对于 MP3、FLAC 等压缩音频，如果你在设置时允许，它会使用电脑上已有的 ffmpeg（绝不下载）。每次启动前，Kumi 都会确认它仍是你允许的那个 ffmpeg 文件（不检查它加载的库，这些库可能随升级而变化）。ffmpeg 只会拿到 Kumi 正在读取的那一个文件：绝不是 URL 或播放列表，也不从网络读取任何内容。Kumi 可读取的文件最大 50 MiB，最多测量其中 12 分钟。
- Kumi 的 REAPER 扩展会把每个已保存工程的 ID 存在工程里（ext state），Renoise 工具会把每首歌的 ID 存在歌曲的工具数据中，并随之一起保存。
- 在 Windows 上，网络文件夹（`\\server\share`，或通过链接访问的共享文件夹）中的采样无法在 Hermes Desktop 或仪表盘中播放：它们只播放这台电脑本地磁盘上的文件。
- 在 Hermes Desktop 或 Hermes 的终端界面中停止，或在聊天应用中发送 `/stop` 或 `/new`，会取消正在进行的 Kumi 修改、撤销或音轨聆听。之后 Kumi 无法确定它是否已完成，所以再次请求前请先查看 HISTORY。在经典的 `hermes chat` CLI 中，停止不会传到 Kumi：调用会一直运行到完成或达到时间上限。
- Kumi 只能在做出修改的那个 Kumi 运行时仍在运行时撤销该修改。运行时会在对话结束、Hermes 重新加载插件，或 15 分钟没有任何操作后停止；之后修改仍留在 HISTORY 中，但要撤销请使用 DAW 自己的撤销。
- 在 Live 中，如果 Kumi 修改了某条音轨的调音台设置、速度或某个设备的参数，而你之后又手动改过这个值，Kumi 不会撤销那次修改：撤销会说明这一点，并且不做任何改动。撤销在征求同意前，会列出每个值的当前值，以及撤销后会恢复成的值。
- Kumi for Hermes 不包含任何会绕过 Hermes 确认的功能：在 DAW 内运行代码、操作 Live 的菜单、录音，以及保存或打开工程。

## 遇到问题时

- `hermes kumi doctor`（在对话中用 `/kumi doctor`）会检查 Kumi 在这个 Hermes 配置中需要的一切：Hermes 的批准设置、设置、连接、打开的项目和声音地图。对每个问题，它都会告诉你该怎么做。它不会更改任何内容。
- `/kumi`（在终端中用 `hermes kumi status`）会告诉你设置还缺什么，以及 Kumi 能否读取压缩音频。当 `plugins.isolation` 为 `host` 时，Hermes 不会加载 `hermes kumi` 这类插件命令，请改在 Hermes Desktop 的对话中设置。
- 聆听 REAPER 或 Renoise 音轨时提示 Kumi 的 DAW 组件太旧：再运行一次设置，然后重启 DAW。
- Hermes Desktop 中看不到 Kumi：检查 技能与工具 → 插件 里 Kumi 的 **Agent** 和**桌面**开关是否打开，以及 Hermes 中是否已启用（`hermes plugins enable kumi`）。
- 网页仪表盘中没有 Kumi 标签页：如果安装 Kumi 时仪表盘已经在运行，请重启它（`hermes dashboard`）。
- Live 连不上：检查是否已把 AbletonMcpBridge 选为控制界面。
