# 支持的平台

[English](../en/SUPPORT_MATRIX.md) · 简体中文 · [日本語](../ja/SUPPORT_MATRIX.md)

Kumi 及其各个桥接能在哪些系统上运行、适用于哪些版本的 Live、REAPER、Renoise 和 Logic Pro，以及在哪里测试过什么。每一项“已测试”背后的证据列在[实现状态](IMPLEMENTATION_STATUS.md)中。

## Kumi

| 系统 | 版本 | 处理器 | 状态 |
| --- | --- | --- | --- |
| macOS | 13（Ventura）或更高 | Apple 芯片、Intel；Logic Pro：仅限 Apple 芯片 | 原生构建和安装纳入 CI；较早的 TypeScript 版已在 Apple 芯片上配合 Live 测试；Logic Pro 已在 Apple 芯片上测试 |
| Windows | 10 或 11 | x64、ARM64 | 原生构建和安装纳入 CI；较早的 TypeScript 版已在 Windows 10 上配合 Live 测试；见 [Windows](#windows) |
| Linux | glibc 2.35 或更高（Ubuntu 22.04、Debian 12 或更新；不包括 Alpine 或其他 musl 发行版） | x64、ARM64 | 原生构建和安装纳入 CI；Live 没有 Linux 版，但 REAPER 和 Renoise 可以在上面运行 |

- Kumi 和独立桥接都是原生 Rust 程序。全新安装不会下载 Node，也不要求 Node。
- 使用 Node 24 的现有 Kumi 1.7.4 和 1.7.5 可通过 `kumi update` 迁移，保留原有设置、登录和数据。保留的 Node 用于回滚和可选的 YouTube 挑战处理。在 Windows 上，原生版首次启动时会替换旧启动器，之后的启动不再经过 Node。更早的 Node 主版本可能需要使用相同 `KUMI_HOME` 重新运行安装程序；见[更新说明](KUMI_GUIDE.md)。
- YouTube 挑战处理使用保留的 Node 或 PATH 中的 Node；Kumi 不会为此下载 Node。Live 扩展由 Live 自己的 JavaScript 宿主运行。

下述真实 Live 测试来自较早的 TypeScript 版本。原生版 CI 和迁移检查不等同于真实 Live 硬件验收。

## Ableton Live

| Live | 状态 |
| --- | --- |
| 12.4 或更高 | 全部功能，包括 Kumi 的 Live 扩展：离线渲染、把 MIDI 片段写进编曲视图、清除一段范围，以及 **Ask Kumi about this**。已在 macOS 上的 Live 12.4.15 beta 中测试。 |
| 12.0 至 12.3 | 桥接提供该版本 Live 的 API 所具备的功能；没有扩展，因此上述功能都不可用。`kumi doctor` 会说明这一点。未测试。 |
| 11 或更早 | 不支持。 |

版本类型：桥接会探查它所连接的 Live 提供了什么，因此某个版本类型缺少的设备和内容（Standard 和 Intro 较少）会保持不可用，而不会靠猜测。制作 Max for Live 设备需要 Max for Live（Suite，或加装了该附加组件的 Standard）。Willington（从带有它的文件的版本起在 Kumi 的桥接中，在此之前可以自己安装；在 `/willington` 开启之前一直关闭）有适用于 macOS ARM64 上 Live 12.4.15b4 和 b5（机架链区域仅限 b5）以及 Windows x64 上 Live 12.4.15b5 的绑定；见 [Willington](WILLINGTON_INTEGRATION.md)。

原生编辑（分组、编曲自动化、场景/全局 Follow Actions、逐音符 MPE）要求安装已验证的 `WillingtonEditing`、通过与实际库匹配的自测，并明确启用。仅支持 Live 12.4.15b5 macOS ARM64，不支持 b4、Intel macOS 或 Windows。参见[自测流程](WILLINGTON_INTEGRATION.md#原生编辑及其自测)。

## REAPER 与 Renoise

| DAW | 版本 | 系统 | 状态 |
| --- | --- | --- | --- |
| REAPER | 7 或更高 | macOS（Apple 芯片和 Intel 共用一个扩展）、Windows x64（Windows on Arm 上的 REAPER 加载 x64，也用同一个扩展）、Linux x64 和 ARM64（glibc 2.35 或更高） | 已在 macOS 上用 REAPER 7.82（评估版、便携版）测试。CI 在每个系统上构建扩展、检查它，并用系统自己的加载器加载它。尚未在 Windows 或 Linux 上的 REAPER 中试用；Windows on Arm 未测试 |
| Renoise | 3.5 或更高 | macOS、Windows、Linux | 已在 macOS 上用 Renoise 3.5.4 测试。CI 针对 Renoise API 的模拟运行工具的测试，并对照 Renoise 自己的 API 定义进行检查。尚未在 Windows 或 Linux 上的 Renoise 中试用 |

- Linux 对两者来说都是 DAW 平台：和 macOS、Windows 一样，设置会在那里连接 REAPER 和 Renoise。
- 在每个系统上，CI 都会通过 Kumi 自己的命令，把两个桥接装进临时的 DAW 文件夹，更新它们，回滚两次，再卸载。
- Renoise 的试用版无法渲染，所以 Kumi 在那里无法聆听歌曲。

## Logic Pro

| Logic Pro | 状态 |
| --- | --- |
| 12，在搭载 Apple 芯片的 Mac 上 | 通过 Kumi 的 Logic Pro 桥接支持（[连接 Logic Pro](KUMI_GUIDE.md#连接-logic-pro)）：App Store 的 Logic Pro（`com.apple.logic10`）和 Creator Studio 的 Logic Pro（`com.apple.mobilelogic`）。已在 macOS 26.6.1 上用 App Store 的英文版 Logic Pro 12.3.1 测试。 |
| 11 或更早 | 不支持；`kumi --daw logic doctor` 会说明这一点。 |
| Intel Mac、Linux、Windows | 没有 Logic Pro 桥接。Logic Pro 12 只能在搭载 Apple 芯片的 Mac 上运行，所以只有搭载 Apple 芯片的 Mac 上的 Kumi 带有这个桥接；其他的 Kumi 会说“Logic Pro is supported on Apple Silicon Macs only”，并且什么都不安装。 |

这个桥接是 Kumi 的一个程序，在 Logic Pro 旁边运行，位于 Kumi 自己的文件夹中：它通过 macOS 的辅助功能（对运行 Kumi 的应用授权一次即可）、Logic Pro 的快捷键命令和菜单，以及虚拟 MIDI 端口上的一个 Mackie Control 控制界面来读取和操作 Logic Pro，并通过 Core Audio 的进程音频分接（process tap）聆听 Logic Pro，这需要 macOS 14.2 或更高版本（以及在 macOS 要求时的 System Audio Recording 权限）。其他语言的 Logic Pro 未经测试：桥接按 Logic Pro 自己的译文读取它的标签，而这些译文只在德语上手动核对过。Kumi 在 Logic Pro 中覆盖了什么，见[能力矩阵](CAPABILITY_MATRIX.md#logic-pro)；在真实 Logic Pro 上的运行，见[实现状态](IMPLEMENTATION_STATUS.md#在哪里测试过什么)。

## Windows

已测试：在 CI 上，于 Windows PowerShell 5.1 和 PowerShell 7 中测试了安装程序、`kumi update`、`kumi bridge` 和 `kumi uninstall`，并在名称含非 ASCII 字符的文件夹中的 `KUMI_HOME` 下测试了安装程序、`kumi bridge`、回滚和 `kumi uninstall`；在一台装有 Live 12.4.15 beta 的 Windows 10 电脑上，测试了用 `kumi bridge` 安装到已移出用户文件夹的 User Library、Remote Script 在 Live 中加载，以及 Kumi 连接。CI 的 Windows 运行器使用默认文件夹的管理员账户，因此无法暴露只在普通账户或移动过的库中才会出现的问题。

尚未在 Windows 上确认：

- **Live 把 Extensions 文件夹放在哪里。** Kumi 使用 `%LOCALAPPDATA%\Ableton\Extensions`；`KUMI_LIVE_EXTENSIONS_DIR` 可以覆盖它。在确认之前，扩展的各项功能在 Windows 上都未经测试。
- **全屏应用在 Windows 终端中的表现。** 推荐使用 Windows Terminal；见[终端](KUMI_TUI.md#终端)。
- **从 Kumi 1.6.0 或更早版本运行 `kumi update`** 时，如果 PATH 上 Git 的 `tar` 排在 Windows 自带的 tar 前面（例如在从 Git Bash 启动的 PowerShell 中），会因 tar 错误而失败。请重新运行那行安装命令，或在 `kumi update` 之前运行 `$env:Path = "$env:SystemRoot\System32;$env:Path"`。

## 旧安装所用的 Node.js

| Node.js | 状态 |
| --- | --- |
| 22.x、24.x | 支持；推荐 Node 24 LTS |
| 25.x | 不支持：已于 2026 年 6 月 1 日终止维护 |
| 26.x 及更高、21.x 及更早、预发布版 | 经过测试之前不支持 |

此表适用于 Kumi 1.7.5 及更早版本的安装，它们运行在 Node 上。其 npm engines 范围为 `>=22 <23 || >=24 <25`；它们的 `kumi` 会拒绝其他主版本（`kumi doctor` 除外，它会说明问题所在）。它们的桥接服务器和 `ableton-mcp-setup` 同样会拒绝；`ableton-mcp-diagnostics` 会报告它们；`ableton-mcp-lifecycle` 和 `ableton-mcp-migrate` 仍可运行，以便检查或移除旧的安装。

## MCP 协议

桥接通过 stdio 使用两代 MCP 协议：`2025-11-25`（使用 initialize 握手）和 `2026-07-28`（使用逐请求元数据和 `server/discover`）。在新一代协议中，每个结果都是完整的，缓存提示为 private 且 TTL 为零，也不会在未经请求时推送任何内容；不提供 MRTR、Tasks 或 HTTP。测试覆盖了这两代协议；没有对任何特定的 MCP 客户端或模型进行认证。如何连接客户端见[用户指南](USER_GUIDE.md)。

## 无障碍

`KUMI_UI=plain`（或把输出通过管道传出）会让 Kumi 使用逐行输出的纯文本界面，适合屏幕阅读器；见[纯文本模式](KUMI_TUI.md#纯文本模式)。桥接自身的输出是顺序固定的纯文本，没有仅靠颜色区分的状态，也没有需要鼠标指针的操作。两者都尚未用 VoiceOver 或 Narrator 测试过；Live、插件窗口和 MCP 客户端的表现则由各自的开发者决定。

## CI 覆盖的范围

CI 在 GitHub 托管的 macOS、Ubuntu 和 Windows 运行器上运行 Rust 构建和测试、Remote Script 和 Live 扩展的测试、六个目标平台的发行包，以及安装和迁移测试，还会在每个系统上加载 REAPER 的扩展，运行 Renoise 工具的测试，并在 macOS 上运行 Logic Pro 桥接的测试（基于录下的场景和一个模拟的 Logic Pro）。这些运行器都没有 Live、REAPER、Renoise 或 Logic Pro。[测试](TESTING.md#ci)列出了每个作业。
