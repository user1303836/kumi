# 发布与分发

[English](../en/DISTRIBUTION_POLICY.md) · 简体中文 · [日本語](../ja/DISTRIBUTION_POLICY.md)

Kumi 及其桥接如何送到用户手中，这能证明什么、不能证明什么，以及桥接的包可以包含哪些内容。制作发布版本的步骤见[开发者指南](DEVELOPER_GUIDE.md#发布)。

## 渠道

| 内容 | 位置 | 获取方式 |
| --- | --- | --- |
| Kumi | `user1303836/kumi` 的 GitHub Releases：原生 `kumi-<target>.tar.gz`、兼容包 `kumi.tar.gz`、`kumi-release.json` 和 `SHA256SUMS`，由 Installer 工作流附加到每个 `vX.Y.Z` 标签上 | `install.sh` 或 `install.ps1`，之后用 `kumi update` |
| 桥接（`@ableton-mcp/mcp-server`） | 包含在每个 Kumi 发行包中：既有原生桥接 tarball，也有已解包的桥接包 | `kumi bridge`，它通过桥接的生命周期进行安装（[安装桥接](DELIVERY.md)） |
| 单独的桥接 | 没有自己的发布版本。用 `python3 scripts/build-native-release.py --bridge-only` 构建（[构建选项](DEVELOPER_GUIDE.md#发布)） | 生命周期 CLI（[安装桥接](DELIVERY.md#独立桥接)） |

安装脚本从 `main` 分支读取；它们安装的发行包来自最新的已发布版本（或 `KUMI_VERSION` 指定的版本）。在维护者发布之前，发布版本只是草稿，只有已发布的版本才是“latest”。不会向 npm 发布任何内容：每个包都是 `private: true`，所以 `npm publish` 会拒绝。

## 只证明完整性，不证明身份

应用和桥接没有发布者签名或公证，也没有 `.pkg`、`.msi` 安装程序。macOS 的 Hands 辅助程序使用临时签名（ad-hoc），这不证明发布者身份。安装程序和 `kumi update` 对照 `kumi-release.json` 中的 sha256 检查发行包。全新原生安装不会下载 Node。`kumi bridge` 对照构建发行包时记录的哈希检查桥接 tarball。与下载内容来自同一处的校验和只能证明字节完整送达，不能证明是谁制作的。

本软件采用 [MIT 许可证](../../LICENSE.md)。该许可证不授予任何 Ableton 商标权利，Kumi 与 Ableton 没有关联，也未获其认可。

## 桥接的包可以包含什么

- 原生可执行文件 `ableton-mcp-server` 和 `ableton-mcp-analysis-worker`（Windows 使用 `.exe`）；
- Remote Script 及其 README、操作注册表，以及它们的哈希清单；
- Kumi 的 Live 扩展：它的清单、`package.json`、构建好的 `extension.js` 以及该文件的 sha256；
- 桥接的指南（`README.md` 和 `release-docs/`）；
- `release-manifest.json`、`package.json` 和 `LICENSE.md`。

除此之外别无他物：没有构建脚本、测试夹具、`node_modules`、凭据、配置、本地状态、日志、捕获的媒体或证据。原生构建器和生命周期会严格核对 `release-manifest.json` 中的文件清单和哈希。

## 发布清单

`release-manifest.json`（schema 为 `ableton-mcp-native-release/v1`）记录包名和版本、源码提交及工作树是否有未提交的修改、Rust 目标平台、rustc 和 Cargo 版本、运行器镜像、`Cargo.lock` 和 CI 工作流的 SHA-256、构建方法、协议注册表哈希，以及每个载荷文件的角色和 SHA-256。

分发字段为 `channel: "local-native-tarball"`，`published`、`signed`、`notarized` 和 `integrityIsIdentityProof` 均为 `false`。生命周期要求这些值。tarball 根据哈希从本地路径安装，随 GitHub Releases 上的 Kumi 发行包送到用户手中，不发布到包注册表。

为支持现有安装的升级和回滚，生命周期也接受旧 schema `ableton-mcp-release/v2` 和 `ableton-mcp-private-release/v1`。这些清单的 Node/npm/TypeScript 构建记录及 `local-npm-tarball` 渠道描述的是 Kumi 1.7.5 及更早版本的桥接包。

## 合并门禁

`main` 分支有一套规则集：

- 修改通过拉取请求进入；不要求批准性审查；
- 一项必需的检查 `Required CI`，必须在分支与 `main` 保持同步的状态下通过；
- `main` 不能被删除或强制推送；
- 仓库管理员角色可以对拉取请求绕过这些规则。

Installer 工作流不是必需的检查，但在标签上，它的 `publish` 作业只有在发行包已在 macOS、Linux 和 Windows 上安装成功之后才会运行。[测试](TESTING.md#ci)介绍了每个作业。

## 待所有者决定的事项

- Kumi 在 macOS 和 Windows 上的发行包和安装程序的**签名与公证**。
- **再分发 Extensions SDK。** Kumi 的 Live 扩展是用本地提供的预发布版 Ableton Extensions SDK 构建的；由于其许可证限制再分发该 SDK，仓库从不提交它。构建出的 `extension.js` 把扩展与它用到的 SDK 代码打包在一起，并且会被提交，随桥接的包和 Kumi 发行包一起发布。这是否被允许，需要由所有者决定。
- `main` 规则集上的**管理员绕过**：保留还是移除。
