# 在 Hermes Agent 中使用 Kumi

[English](../en/HERMES.md) · 简体中文 · [日本語](../ja/HERMES.md)

Kumi 也是 [Hermes Agent](https://hermes-agent.nousresearch.com) 的插件。智能体仍然是 Hermes（终端、Hermes Desktop、网页仪表盘或聊天应用），Kumi 为它接上你的 DAW。每次修改前 Hermes 都会先征求你的同意，Kumi 把每一处修改记入 HISTORY 并附带撤销。两者都在你的电脑上运行。

单独的 Kumi（在终端中使用的 `kumi` 应用）和 Hermes 中的 Kumi 是两套独立的安装，只用其中一个也可以。

## 安装

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

在 Hermes Desktop 中，还要在 **Capabilities → Plugins** 里打开 Kumi：插件的 Desktop 部分默认是关闭的。

## 设置

让 Hermes“为 Live 设置 Kumi”（REAPER 或 Renoise 也一样），或在终端运行 `hermes kumi setup`。设置分两步，每一步之前 Hermes 都会征求你的同意：

1. **Kumi 的运行时：** 从 Kumi 的 GitHub 发布页下载适用于你电脑的 Kumi 官方版本（25–32 MB），用插件固定的 SHA-256 校验，再解压到 Hermes 为这个配置保存的数据中（77–113 MB）。
2. **DAW 里的 Kumi 部分：** Live 的 Remote Script（含用于聆听音轨的 Kumi Ears 设备）、REAPER 的扩展或 Renoise 的工具，安装到设置向你展示的确切位置。

然后在 DAW 中：
- **Live：** 在设置 → Link, Tempo & MIDI 中把 **AbletonMcpBridge** 选为控制界面。
- **REAPER：** 重启 REAPER。
- **Renoise：** 启用 Kumi 工具。

如果单独的 Kumi 已经在你的 DAW 里安装了它的部分，设置会告诉你，并建议直接使用那套安装而不是替换它。这样 Hermes 中的 Kumi 会读取那套安装的连接文件，并把自己的连接状态写在那里，但绝不使用它的登录信息或设置。

设置不会启动或退出你的 DAW、替你选择控制界面，也不会保存你的工程。

## 可以让它做什么

- “我现在在看什么？”Kumi 读取你选中的音轨、设备和片段。
- “帮我找一个更暗的军鼓。”Kumi 在你列出的采样文件夹中查找；在 Hermes Desktop 中会打开声音地图。
- “把它放到第 2 个打击垫上。”Hermes 先准确说明会改变什么并征求同意，Kumi 再做修改并记入 HISTORY。
- “真的更暗了吗？”Kumi 测量两个文件：响度、真峰值、频谱。
- “撤销军鼓那次修改。”只要还能撤销，Kumi 就撤销那一条。

## 东西在哪里

- **所有界面：** Kumi 的工具、技能（`kumi:daw`、`kumi:sounds`、`kumi:listening`）以及查看状态的 `/kumi`。在消息中写 `@kumi:focus`、`@kumi:history` 或 `@kumi:status`，即可附上 Kumi 最后看到的内容。
- **Hermes Desktop：** 状态标签、显示 FOCUS 和 HISTORY 的侧边栏、单独标签页中的声音地图、测量结果卡片和命令面板中的命令。按钮只会把请求放进对话，不会自己修改你的工程。
- **网页仪表盘：** 一个包含 HISTORY、状态和地图的 Kumi 标签页。

## 设置项

| 设置 | 默认 | 作用 |
|---|---|---|
| DAW | Live | Kumi 连接哪个 DAW |
| 采样文件夹 | 无 | 为声音地图允许 Kumi 读取的文件夹，其他地方一概不扫描 |
| 告诉智能体我在看什么 | 开 | Kumi 连接期间，在每一轮加入一段简短的 FOCUS 和变化摘要 |
| 解读测量结果 | 关 | 把测量数值和一张小的频段能量图也发给你的 Hermes 模型，用文字解读 |

## 哪些内容会离开你的电脑

- **设置：** 每个 Kumi 版本从 Kumi 的 GitHub 发布页下载一次运行时。
- **对话：** Kumi 记下的工程信息（FOCUS、变化）和测量结果，会作为对话的一部分发给你在 Hermes 中使用的模型。
- **解读测量结果**（只有打开后才会）：把测量数值和一张小的频段能量图发给你在 Hermes 中为 Kumi 聆听任务选择的模型，或 Hermes 为该任务配置的后备服务商，绝不发送音频本身或文件路径。重试和后备可能增加时间和 token 费用。
- **音频留在本地：** Kumi 在你的电脑上测量音频，不会把音频发给任何模型。在聊天应用（Telegram、Discord 等）中，如果你让 Hermes 把声音发给你，它会作为附件通过该应用送达。
- **聆听音轨或混音**之前会先征求同意，只播放你批准的节拍范围（最多 60 秒），而且只在 DAW 停止时进行。路由和走带可能会短暂改变，结束后会恢复。Live 通过 Kumi Ears（Max for Live）采集；REAPER 和 Renoise 渲染到拥有其 DAW 部分的那套 Kumi 安装的 listening 文件夹中，Kumi 只复制那一个文件，然后删除它。在 REAPER 中聆听音轨后，工程可能显示为已修改（只有保存才能清除这个标记，而 Kumi 从不保存）。
- Hermes 中的 Kumi 绝不使用 Kumi 自己的登录、模型服务商或更新程序，也不发送遥测数据。

## 限制

- 你的 DAW 必须运行在运行 Hermes 的那台电脑上。连接到远程 Hermes 的 Hermes Desktop 控制的是那台电脑上的 DAW，而不是你本机的。
- Live 支持 macOS 和 Windows；REAPER 和 Renoise 支持 macOS、Windows 和 Linux。
- Hermes 中的 Kumi 不包含任何会绕过 Hermes 确认的功能：在 DAW 内运行代码、操作 Live 的菜单、录音，以及保存或打开工程。

## 遇到问题时

- `/kumi`（在终端中用 `hermes kumi status`）会告诉你设置还缺什么。
- Hermes Desktop 中看不到 Kumi：检查 Capabilities → Plugins 里是否打开了 Kumi，以及 Hermes 中是否已启用（`hermes plugins enable kumi`）。
- Live 连不上：检查是否已把 AbletonMcpBridge 选为控制界面。
- 聆听 Live 音轨时提示缺少 Kumi Ears：再运行一次设置。如果 Hermes 中的 Kumi 使用的是另一套 Kumi 安装，请改在那套 Kumi 中运行 `kumi bridge`。Kumi Ears 需要 Max for Live。
