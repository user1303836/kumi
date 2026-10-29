<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="REAPER 7" src="https://img.shields.io/badge/REAPER-7-111111">
  <img alt="Renoise 3.5" src="https://img.shields.io/badge/Renoise-3.5-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文 · <a href="README.ja.md">日本語</a>
</p>

**Kumi 是一个通用 AI 代理，在你的 DAW 里与你并肩工作。** 它目前支持 Ableton Live、REAPER 和 Renoise，在每个 DAW 中的能力都一样：读取你的工程，几乎可以修改其中的任何东西，聆听结果，并学习你的声音、口味和习惯。你在终端里运行的聊天中用平常的话提出要求；Kumi 规划修改、执行修改，并把每一步连同撤销一起记在它的历史记录里。你用 Kumi 越久，它就越熟悉你的工作方式、你的偏好，以及你作为制作人的习惯。

https://github.com/user-attachments/assets/f53fe0be-c9a9-476e-a1d5-efe3dc429526

## 快速开始

在 Mac 的终端或 Windows 的 PowerShell 中，一条命令即可开始。

Mac 或 Linux：

```sh
curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh
```

Windows：

```powershell
irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex
```

按照 Kumi 的引导完成设置。

之后，打开终端并输入 `kumi`，再打开你的 DAW（Live、REAPER 或 Renoise，在 Kumi 之前或之后打开都可以）：Kumi 会连接到开着的那个。

> [!NOTE]
> Kumi 在终端窗口中运行，但你不需要会任何命令。运行完安装的那一行，剩下的就是聊天。拖动选中文字后，按 Ctrl-C 即可复制。

## 你可以让 Kumi 做什么

- **照着教程做出声音。** 粘贴一个 YouTube 链接：Kumi 会观看视频，并在新轨道上搭建出其中展示的内容。
- **修正混音。** 问一句“贝斯是不是太浑？”，Kumi 会聆听、找出打架的部分并修正，每次修改后都重新聆听。
- **匹配参考曲。** 给它一首你喜欢的曲子：Kumi 会尝试几个版本，逐一与它对比打分，再精修最好的那个。
- **创建插件预设并进行母带处理。** Kumi 使用内置 VST3 格式，并在原生设备之前检查合适的已安装插件。Kumi 可以在后台创建预设，让你继续手头的工作。[支持的格式与限制](plugin-formats/README.md)。
- **制作设备。** 描述一个 Max for Live 效果器或乐器，Kumi 就会把它做出来并放到你的轨道上。
- **找到你的声音。** “像这个一样带点颗粒感的军鼓”“我常用的人声效果链”：Kumi 了解你的采样、预设和工程。
- **把声音画成地图。** `/map` 把你的采样和歌曲排列出来，相似的彼此相邻；指向一个点就能试听。把喜欢的声音收进调色板，当你说“从我的调色板里找一个更暗的底鼓”时，Kumi 就从中挑选，也可以把整个调色板作为一套鼓组载入。
- **处理杂活。** 重命名、上色、路由、侧链、编组、冻结、并轨和导出。

它在三个 DAW 中的工作方式相同，各用该 DAW 自己的说法：[在每个 DAW 中能做什么](docs/zh-CN/DAWS.md)。

## 常见问题

<details>
<summary><b>需要会写代码吗？</b></summary>

不需要。你用平常的话与 Kumi 交流，打字或说话都可以。需要代码的工作（Live 中的 Python，REAPER 和 Renoise 中的 Lua），Kumi 会自己编写并运行。

</details>

<details>
<summary><b>它会弄坏我的工程吗？</b></summary>

每项修改都会列在 Kumi 的历史记录中（“Tempo 120 → 124 BPM”），大多数都有各自的撤销。在 Live 中，每个请求也只算 Live 自身撤销中的一步；在大规模或带有删除的修改之前，Kumi 会复制一份你最后保存的 Set。

</details>

<details>
<summary><b>支持哪些 DAW 和电脑？</b></summary>

- Ableton Live 12，运行在 macOS 13 或更高版本，或 Windows 10、11 上
- REAPER 7 及更高版本，运行在 macOS、Windows 或 Linux 上
- Renoise 3.5 及更高版本，运行在 macOS、Windows 或 Linux 上

装了不止一个时，设置会问你用哪一个；之后可以用 `/daw` 切换。

</details>

<details>
<summary><b>它用什么 AI？要花多少钱？</b></summary>

Kumi 是免费的开源软件（MIT）。模型由你选择：

- **ChatGPT 套餐：** 用你的账户登录；Kumi 在你套餐的用量限制内工作。
- **API 密钥：** Anthropic、OpenAI、Google Gemini、OpenRouter 以及[另外 20 家提供方](docs/zh-CN/KUMI_GUIDE.md#登录并选择模型)，由提供方计费。
- **本地：** Ollama、LM Studio，或 llama.cpp、vLLM 等兼容 OpenAI 的服务器；所用硬件要能以较长的上下文（Kumi 会请求约 57k token）运行支持工具调用的模型。

</details>

<details>
<summary><b>我的音频会离开我的电脑吗？</b></summary>

音频在本地分析，模型拿到的是测量结果。你的素材库和音乐收藏也在本地学习。有 Gemini 或 OpenAI 的 API 密钥时，聆听模型还会听到它所判定修改的单声道片段（最长 10 秒）；`/slots listening off` 可以关闭它。其余内容见[隐私](docs/zh-CN/KUMI_GUIDE.md#隐私哪些内容会离开你的电脑)。

</details>

<details>
<summary><b>遇到问题时</b></summary>

`kumi doctor` 会检查安装、登录、桥接以及与 DAW 的连接，并告诉你该怎么修。`kumi report` 会生成一个文件，可以附在 [issue](https://github.com/user1303836/kumi/issues) 中。

</details>

[指南](docs/zh-CN/KUMI_GUIDE.md) · [命令、按键与界面](docs/zh-CN/KUMI_TUI.md) · [Kumi 如何修改你的工程](docs/zh-CN/KUMI_CHANGES.md) · [更新日志（英文）](CHANGELOG.md)

## 当前状态

Kumi 1.24.15 已在 macOS 的 Ableton Live 12.4（测试版）中验证。
在 Windows 上，安装和升级已经过验证，但与 Live 一起使用还是新的。
出问题时请发送 `kumi report`。
REAPER 7.82 和 Renoise 3.5.4 已在 macOS 上验证。在 Windows 和 Linux 上，CI 验证了装入 Kumi 的扩展和工具以及加载该扩展，但在那里配合 REAPER 和 Renoise 使用还是新的。

## 开发

要从源代码构建 Kumi，请参阅 [DEVELOPMENT.md](DEVELOPMENT.md)（英文）和[开发者指南](docs/zh-CN/DEVELOPER_GUIDE.md)。

## 许可证

[MIT](LICENSE.md)。来自他人成果的部分，以及 Kumi 下载的模型，见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。Ableton Live 是 Ableton AG 的商标；Kumi 与 Ableton、Cockos（REAPER）、Renoise 及兼容插件的制造商均无隶属、关联、赞助或背书关系；产品名称仅用于标识兼容软件。
