# 实现状态

[English](../en/IMPLEMENTATION_STATUS.md) · 简体中文 · [日本語](../ja/IMPLEMENTATION_STATUS.md)

当前进展：现行版本、在真实 Live 上测试过什么以及记录在哪里，还有已知的限制。Live 各个领域支持什么，见[能力矩阵](CAPABILITY_MATRIX.md)；系统和 Live 版本见[支持的平台](SUPPORT_MATRIX.md)。

## 版本

[更新日志](../../CHANGELOG.md)中的每个版本都写明了随附的桥接；[桥接版本](KUMI_CHANGES.md#桥接版本)列出了全部桥接版本。版本号本身记在各 crate 的 `Cargo.toml` 文件（Kumi、桥接）、根目录的 `package.json`（Kumi）和 `apps/live-extension/manifest.json`（扩展）中。

| 组成部分 | 版本 |
| --- | --- |
| Live 协议 | `ableton-live/v1`；每个发布版本的 `release-manifest.json` 都记录了注册表的哈希 |
| MCP 协议的两代 | `2025-11-25` 和 `2026-07-28` |
| 运行时 | 应用和独立桥接都是原生 Rust；全新安装无需 Node |
| 旧安装（Kumi 1.7.5 及更早版本） | Node 22/24；保留的 Node 也用于回滚和可选的 YouTube 挑战处理（[详情](SUPPORT_MATRIX.md)） |

## 在哪里测试过什么

以下真实 Live 记录来自较早的 TypeScript 发布版本。原生版的 CI 和迁移测试不能替代新的真实 Live 验收。

- **每个拉取请求：** 原生 Rust 构建和测试、Remote Script 和 Live 扩展的测试、六个目标平台的发行包，以及安装和迁移测试；见 [CI](TESTING.md#ci)。
- **macOS 上的真实 Live**（Apple 芯片，Live 12.4.15 beta）：Kumi 做出的每一类修改，且每一项都通过 Kumi 撤销（在桥接 1.0.62 和 1.0.63 上 65 项全部通过，分别在 19 条和 200 条轨道的工程中），播放、并轨（bounce）、聆听和观看，通过 Kumi 扩展进行的离线渲染和右键菜单，以及 Willington 编辑。
- **Windows 上的真实 Live**（Windows 10，Live 12.4.15 beta，Kumi 1.6.0 配合桥接 1.0.71）：把桥接安装到移动过的 User Library、Remote Script 加载，以及 Kumi 连接。这部分目前还没有记录文件。

## 证据

记录在 [`docs/evidence`](../evidence/) 中。如果某条记录没有写明桥接版本，表中给出的是添加该记录的那次提交中的版本。

| 记录 | 日期 | Live | 桥接 | 说明的内容 |
| --- | --- | --- | --- | --- |
| [kumi-poc.md](../evidence/kumi-poc.md) | 2026-09-28 至 09-30 | 12.4.15b4、b5 | 1.0.0 至 1.0.63 | 真实 Live 上的 Kumi：验收运行、速度与大型工程、`kumi bridge`、试听与目标、一个 Max for Live 设备、重新连接 |
| [live-extension.md](../evidence/live-extension.md) | 2026-09-30 | 12.4.15b5 | 1.0.57 至 1.0.65 | Live 如何运行 Kumi 的扩展；离线渲染及其开销、两个通道结果一致、撤销、右键菜单 |
| [lom-audit.md](../evidence/lom-audit.md)、[JSON](../evidence/lom-audit-12.4.15b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.55 | 对 Live 的 Python API 所做的普查，与 Remote Script 实际使用的内容对照 |
| [kumi-benchmark.md](../evidence/kumi-benchmark.md) | 2026-09-30 | 12.4 beta | 1.0.52 | 凭耳朵重建一首曲子中的一个段落：运行与评分 |
| [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.53 | 通过 Willington 设置 Follow Actions 和 Legato，读回并撤销 |
| [willington-kumi-chat.json](../evidence/willington-kumi-chat.json) | 2026-09-30 | 12.4.15b4 | 1.0.52 | 一次使用 Willington 编辑的 Kumi 对话，每项编辑都已撤销 |
| [rack-zones-b5.json](../evidence/rack-zones-b5.json) | 2026-10-01、2026-10-02 | 12.4.15b5 | 1.0.66 | 通过 Willington 处理机架链区域：读取、写入、撤销、保存并重新打开；之后还有信号门控、淡变，以及 Max `live.object` 的写入、读回和恢复 |
| [capability-manifest.json](../evidence/capability-manifest.json) | 截至 Kumi 1.7.6（2026-10-04） | — | 1.0.74 | 每个注册表操作（可执行或保留），以及注册表的哈希；不再重新生成 |
| `phase-3` 至 `phase-9` 的文件 | 2026-07-26 至 07-28 | 12.4.5b8 | 0.1.0 | 桥接在有 Kumi 之前最早的几次真实 Live 运行：探查、试听、走带、片段、编曲视图、调音台、自动化、设备、Browser、路由、录音、项目文件、事件、实时控制和捕获；另有 [FFmpeg 响度对照基准](../evidence/phase-8-audio-oracle.json)，以及针对假 Live 运行的打包用户旅程。这些是历史记录：此后桥接已有很大变化。 |

## 已知限制

- Live 的 API 无法保存或导出工程、冻结、创建编组轨道或编辑编曲视图中的自动化；见 [Live 的 API 不提供的功能](CAPABILITY_MATRIX.md#live-的-api-不提供的功能)。
- 有些修改无法通过 Kumi 撤销，只能用 Live 自己的撤销；见 [Live 安全](LIVE_SAFETY.md)。
- 在 Windows 上，Kumi 的扩展尚未测试，并且从 1.6.0 或更早版本运行 `kumi update` 可能因 tar 错误而失败；见 [Windows](SUPPORT_MATRIX.md#windows)。
- 到目前为止的真实 Live 测试都在 Live 12.4.15 beta 上进行，主要是在 Apple 芯片的 Mac 上。Live 12.0 至 12.3、Intel Mac 和屏幕阅读器都未经测试。
- Willington 编辑在 `/willington` 开启之前一直关闭，并且需要 Willington 有绑定的 Live 构建版本：macOS ARM64 上的 Live 12.4.15b4 和 b5（机架链区域仅限 b5），以及 Windows x64 上的 Live 12.4.15b5。在 Windows 上，这些绑定已通过 Live 中的 Python 测试，但还没有通过 Kumi 自己的编辑测试；见 [Willington](WILLINGTON_INTEGRATION.md)。
- Kumi 和桥接都没有签名；见[发布与分发](DISTRIBUTION_POLICY.md)。

## 待完成的工作

- 支持 Renoise 和 Reaper。
- 确认 Windows 上的 Live 把 Extensions 文件夹放在哪里。
- 生成的能力清单仍把 Browser 预览列为保留，尽管只要 Live 的 Browser 能够预览，桥接就会提供它。
- [待所有者决定的事项](DISTRIBUTION_POLICY.md#待所有者决定的事项)中列出的各项决定。
