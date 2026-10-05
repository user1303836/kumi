<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文 · <a href="README.ja.md">日本語</a>
</p>

**为 Ableton Live 打造、会学习你工作方式的录音室制作人代理。** 用平常的话告诉 Kumi 你想要什么，它就直接在你的工程里动手，从繁琐的杂活，到你一直没时间做的事：照着 YouTube 教程重建一个声音、让你的混音贴近参考曲、编写你描述的 Max for Live 设备，或者改造你指着的那个机架。每项修改都会出现在 HISTORY 中，大多数都有各自的撤销；整个计划在 Live 中只算一次撤销（Cmd-Z，Windows 上为 Ctrl-Z）。Kumi 会记住你保留下来的技巧，所以每次使用都更贴合你。对其他 DAW 的支持已在计划中。

<p align="center">
  <img src="docs/assets/kumi-screenshot.png" alt="Kumi 根据视频教程重建 Drift 贝斯：记录它每一步操作的对话、显示新轨道设备链的 FOCUS，以及每项修改都带撤销的 HISTORY" width="760">
</p>

## 它能做什么

- **修改工程里几乎任何东西：** 速度、音阶与律动；调音台、路由与侧链；轨道、场景与片段；音符与 MIDI 变换；设备、机架及其参数。
- **聆听：** 混音、采样或它自己并轨出的音频的响度、音色平衡、声像宽度、速度与调性，以及你的混音与参考曲的差别。
- **匹配参考曲：** 为一个声音做出几个不同的版本，逐一与参考曲对比打分，再精修最好的那个。`/goal` 会一直做下去，直到达到目标。
- **观看教程：** 观看 YouTube 或本地文件中的教程，然后在新轨道上搭建出教程里展示的内容。
- **播放、录音与重采样**，让你听到它做出来的东西，或检查它自己的工作。
- **全面掌控 Live：** 按你的要求删除内容，把 MIDI 直接写进编曲视图，离线渲染，在 Live 内运行 Python 来处理其他工具够不着的地方，并回答你在 Live 中右键点击的对象（“Ask Kumi about this”）。几百条轨道的大工程也一样快。
- **制作 Max for Live 设备：** 按你用平常的话给出的描述制作设备（MIDI 效果器、音频效果器和乐器），并放到你的轨道上。
- **查找资料：** 搜索网络，阅读网页、PDF、说明书和 GitHub 上的代码，从而能照着读到的资料做出类似的效果器。
- **显示你在哪里：** FOCUS 跟随你在 Live 中触碰的对象，显示为设备树、钢琴卷帘，或 Session、Arrangement 视图的条带。点击某个设备即可指向它：“这个 Saturator 太刺耳了”。
- **记住：** 关于你和每个工程的笔记、从你保留的内容中学到的技巧，以及可重放的配方。每次保存都会显示，点一下就能让它忘掉。
- **保存对话：** 为每个工程保存对话，并告诉你它关闭期间发生了哪些变化。
- **用你的模型：** 使用 ChatGPT 登录，或使用 OpenAI、Anthropic、OpenCode 的 API 密钥。

## 开始使用

需要 Ableton Live 12，运行在 macOS 13 或更高版本，或 Windows 10、11 上。Live 12.4 或更高版本还会加入右键菜单、直接写进编曲视图的 MIDI 以及离线渲染。制作 Max for Live 设备需要 Max for Live（Live Suite，或加装了附加组件的 Standard）。Kumi 作为原生应用运行。

**macOS：** 打开“终端”，粘贴：

```sh
curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/main/install.sh | sh
```

**Windows：** 打开 PowerShell，粘贴：

```powershell
irm https://raw.githubusercontent.com/user1303836/kumi/main/install.ps1 | iex
```

然后在新的终端窗口中（Windows 上用同一个窗口即可）：

```sh
kumi login      # 使用 ChatGPT 登录，或使用 Anthropic、OpenAI、OpenCode 的密钥
kumi bridge     # 在 Live 关闭时：把 Kumi 连接到 Live（只需一次）
kumi            # 在你的工程旁打开 Kumi
```

之后首次打开 Live 时，请在 Live 的 **Settings → Link, Tempo & MIDI** 中把 **AbletonMcpBridge** 选为 Control Surface。之后 Kumi 会自己找到 Live。

在 Kumi 中输入 `/` 查看命令。Esc 停止 Kumi 正在做的事，`/stop` 停止 Live。Kumi 工作时，按 Enter 可以补充说明（它会在当前这一步之后读到），按 Tab 发送一条等它做完再处理的消息，`/btw` 可以顺便问个问题。

遇到问题？`kumi doctor` 会检查所有环节并告诉你该运行什么。`kumi report` 把出错的情况整理成一个可以发给我们的文件。`kumi uninstall` 会卸载 Kumi。

有新版本时，Kumi 会在启动时告诉你。在 Kumi 中输入 `/update`，或在终端运行 `kumi update`，即可获取新版本，桥接也会一并更新；`kumi update --check` 只检查、不安装，`kumi update --rollback` 回到上一个版本。如果不想让它检查，在 `~/.kumi/settings.json` 中加入 `"updateCheck": false`。

从 Kumi 1.7.5 或更早版本升级时，关闭 Live，运行 `kumi update`，然后照常打开 Kumi。
设置、登录信息、对话和素材库都保留原位。首次启动原生应用时，现有桥接也会切换到原生版本。
[迁移与回滚说明](docs/zh-CN/KUMI_GUIDE.md#更新报告与卸载)。

[指南](docs/zh-CN/KUMI_GUIDE.md) · [命令、按键与界面](docs/zh-CN/KUMI_TUI.md) · [Kumi 如何修改你的工程](docs/zh-CN/KUMI_CHANGES.md) · [更新日志（英文）](CHANGELOG.md)

## 当前状态

Kumi 1.7.6 是第一个原生版本，已在 macOS 的 Ableton Live 12.4（测试版）中验证。
在 Windows 上，安装和升级已经过验证，但与 Live 一起使用还是新的。
出问题时请发送 `kumi report`。接下来将支持 Renoise 和 Reaper。

## 开发

使用 Rust 和 Cargo 构建当前检出的代码：

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --                     # 在 -- 后添加 bridge、doctor 等参数
npm ci --prefix crates/kumi-runtime/tests/support  # 只需一次：部分测试运行的官方 SDK
sh scripts/test-isolated.sh                        # 无需 Live 或登录
```

原有的 `npm run setup` 和 `npm run kumi -- ...` 仍可使用。有 Cargo 时，它们构建并运行当前检出的代码。
没有 Cargo 时，它们安装并运行对应版本的已发布原生应用。`~/.kumi` 中的设置、登录信息、对话和素材库
保持原样。迁移后，运行 `kumi` 即可直接启动原生应用。

| 文件夹 | 内容 |
| --- | --- |
| `crates/kumi` | 终端应用和 `kumi` 命令 |
| `crates/kumi-runtime` | Kumi 的代理核心：模型提供方、记忆、音频分析、视频、网络，以及与 Live 的集成 |
| `crates/ableton-mcp-server` | 桥接：由 Kumi 启动的本地 MCP 服务器，也可单独与其他 MCP 客户端配合使用（[桥接指南（英文）](crates/ableton-mcp-server/README.md)） |
| `remote-script` | 桥接的 Remote Script，运行在 Live 内部 |
| `apps/live-extension` | Kumi 的 Live 扩展（Live 12.4 及更高版本） |
| `protocol` | 桥接与 Remote Script 共用的操作列表 |

[开发者指南](docs/zh-CN/DEVELOPER_GUIDE.md)介绍构建、测试与发布。

## 许可证

[MIT](LICENSE.md)。Ableton Live 是 Ableton AG 的商标；Kumi 与 Ableton 没有关联，也未获其认可。
