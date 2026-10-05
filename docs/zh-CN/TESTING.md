# 测试

[English](../en/TESTING.md) · 简体中文 · [日本語](../ja/TESTING.md)

如何运行仓库各部分的测试、它们需要什么，以及 CI 运行什么。所有常规测试都不需要 Live，也不需要登录。

## 快速开始

在仓库根目录运行：

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # 只需一次：部分 Rust 测试运行的官方 SDK
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s remote-script -p 'test_*.py'
npm ci --prefix apps/live-extension
npm test --prefix apps/live-extension
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

它们需要：

| 需要 | 用于 |
| --- | --- |
| Rust 和 Cargo | 各个 crate 和迁移测试 |
| PATH 上的 Node.js（CI 使用 24） | 部分 Rust 测试、Live 扩展的测试和迁移测试 |
| PATH 上的 Python 3.11 或更高版本（`python3`，在 Windows 上为 `python.exe`） | Remote Script 测试、发布脚本的测试，以及会运行 Python 的 Rust 测试 |
| 在 `vendor/` 中本地提供的 Extensions SDK | 构建 Live 扩展或对其做类型检查（它的测试不需要） |

在 Windows 上，有几个测试会创建符号链接，这需要开发人员模式或管理员账户。没有这项权限时，其中一些会跳过，少数会以 `EPERM` 失败；CI 的 Windows 运行器具有这项权限。

## Kumi 与桥接

在仓库根目录运行。

| 命令 | 作用 |
| --- | --- |
| `sh scripts/test-isolated.sh`（Windows：`./scripts/test-isolated.ps1`） | 在独立的主目录中运行 `cargo test --workspace --locked`；参数会传给 `cargo test` |
| `sh scripts/test-isolated.sh -p kumi-runtime --test hands_transport` | 只运行一个 crate 的一个测试文件 |
| `cargo fmt --all --check` | 按 CI 的方式检查格式 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lint 检查；目前在 CI 中仅供参考 |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | 桥接的性能预算；见[开发者指南](DEVELOPER_GUIDE.md#构建测试与测量) |

隔离运行器为测试提供独立的主目录：`HOME`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`、`XDG_CONFIG_HOME` 和 `KUMI_HOME` 都指向一个全新的临时文件夹，`KUMI_REMOTE_SCRIPTS_DIR` 和 `KUMI_LIVE_EXTENSIONS_DIR` 则被移除，因此任何测试都无法触及你的 Live 文件夹或 `~/.kumi`。

有些测试会在 Kumi 自己的客户端和提供方旁边运行官方的 MCP 和模型 SDK；在运行 `npm ci --prefix crates/kumi-runtime/tests/support` 之前，这些测试会失败。响度和真峰值测试会把分析结果与 FFmpeg 的 `ebur128` 对生成音频得出的结果进行比对。

各 crate 测试中的 JSON oracle 文件是从 TypeScript 实现记录下来的基准文件（golden file）；该实现以及生成这些文件的脚本保留在 git 标签 `v1.7.6` 上，当前代码树中没有任何东西能重新生成它们。有意改变行为之后，请根据原生输出改写受影响的 oracle 条目：失败的断言会打印实际得到的结果；对于 SHA-256 条目，写入该输出的哈希。检查差异，并在提交信息中说明基准文件有改动及其原因。宿主测试固定使用记录基准文件时的桥接版本（`ORACLE_VERSION`），所以提升桥接版本号不会改变任何基准文件。

## Remote Script

在仓库根目录运行：

```sh
python3 -m unittest discover -s remote-script -p 'test_*.py'
python3 -m compileall -q remote-script/AbletonMcpBridge
```

这些测试针对假 Live 对象运行 Remote Script，覆盖：认证、顺序控制、主线程队列、注册表及其哈希、探查、事务、捕获与实时安全，以及可选的 Willington 提供方。

## Kumi 的 Live 扩展

`apps/live-extension` 有自己的 `package.json`。运行 `npm ci --prefix apps/live-extension` 之后，`npm test --prefix apps/live-extension` 会针对假 Live 加载已提交的 `dist/extension.js`，并对照记录的 sha256 检查它。在该目录中构建（`npm run build`）和类型检查（`npm run typecheck`）都需要 `vendor/` 中的 Extensions SDK；没有 SDK 时，已提交的构建保持原样。重新构建之后，请把 `dist/extension.js` 连同它的 `.sha256` 一起提交。

## 发布脚本

`python3 -m unittest discover -s scripts/tests -p 'test_*release.py'` 运行打包测试和迁移测试。打包测试检查发行包的内容和版本，以及打包的指南中每个链接都能解析。迁移测试针对原生发行包运行最后一个 JavaScript 版本的更新器。缺少以下变量时，需要它们的测试会跳过：

| 变量 | 指向什么 |
| --- | --- |
| `KUMI_LEGACY_APP` | 解包后的 Kumi 1.7.5：`python3 scripts/fetch-legacy-release.py [folder]` 会下载它、检查其 SHA-256、解包并打印所在文件夹 |
| `KUMI_NATIVE_RELEASES` | 存放已构建发布产物的文件夹（见[发布](DEVELOPER_GUIDE.md#发布)） |

`KUMI_LEGACY_APP` 也用于桥接自己的迁移测试 `crates/ableton-mcp-server/tests/lifecycle_migration.rs`。缺少该变量时，这个测试不会跳过，而是下载已发布的 Kumi 1.7.5 发行包并检查其 SHA-256。设置该变量后，只要发行包已下载过一次，它就可以离线运行。

## 需要 Live 或模型的检查

这些检查需要主动运行。它们会改动真实的东西或消耗真实的 token，所以 CI 不运行它们。

| 命令（在根目录运行） | 需要 | 作用 |
| --- | --- | --- |
| `cargo run --release -p kumi --example accept_live -- --set "<Set>"` | 打开了某个工程的一次性副本的 Live，以及事先构建好的桥接：`cargo build --release -p ableton-mcp-server --bins`（调试运行时，去掉 `--release` 即可） | 做出 Kumi 能做的每一类修改，用 Kumi 的撤销逐一撤销，播放、并轨、聆听和观看，并测量读取大型工程的耗时。不使用模型。 |
| `cargo run --release -p kumi --example eval_changes [-- <part of a case name>]` | 你的登录和模型 | 检验模型如何使用 Kumi 的工具，针对一个合成桥接进行，它带有从原生工具目录读取的真实桥接工具 schema。从不触及 Live。每个用例给出所用时间、其中工具所占的时间，以及调用模型的次数；`EVAL_EFFORT` 设置模型的推理强度，`EVAL_TRACE=1` 逐一打印每次调用。`EVAL_MEASURE=1` 无需登录或模型，按字节打印每个请求都携带的内容：指令和每个工具的定义（`EVAL_MEASURE=tools` 还会打印定义本身）。 |
| `cargo run --release -p kumi --example probe_inference` | 你的登录 | 用一个无害的工具发送一次经认证的请求。从不触及 Live。 |

合成桥接中的 Operator、Saturator 和 EQ Eight 带有 Live 12.4 给它们的全部参数（从 Live 读入 `crates/kumi/examples/fixtures/eval_changes/live-devices.json`），并像 Live 一样运行 Kumi 自己设置参数的脚本。

## 文档

编辑文档之后，运行打包测试：

```sh
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

它们检查桥接随附的指南中的每个链接：桥接的 README，以及 `scripts/build-native-release.py` 的 `DOCUMENTS` 中列出的十四篇 `docs/en` 页面。没有任何检查会核对三种语言是否一致，请手动保持同步。

## CI

每个拉取请求以及每次推送到 `main` 时，都会运行两个工作流：

| 工作流 | 作业 | 运行内容 |
| --- | --- | --- |
| **CI** | `Rust / Linux`、`Rust / macOS`、`Rust / Windows` | `cargo fmt --check`、构建所有目标、在装好官方 SDK 的情况下通过隔离运行器运行全部测试（在 Windows 上先运行控制台输入测试）、Clippy（仅供参考）和 `git diff --check` |
| | `Python Remote Script / ubuntu-24.04`、`macos-15`、`windows-2025`（Python 3.11） | Remote Script 的测试；编译该包 |
| | `Live extension`（Ubuntu，Node 24） | 针对已提交的构建运行扩展的测试 |
| | `Release scripts`（Ubuntu） | 检查本次修改的空白字符，然后运行打包测试 |
| | `Required CI` | 只有以上全部通过时才通过 |
| **Installer** | `Build Kumi's Mac helper`、`Native bundle / <target>`（六个）、`Aggregate native and existing-installer releases`，然后是 `Install / <system>`（六个）和 `Existing installer transition / <system>`（三个） | 构建 Kumi 在 Mac 上使用 Live 菜单的辅助程序（通用、临时签名），为 macOS、Linux 和 Windows 的 Intel 与 ARM 构建原生发行包，再构建现有安装用来更新的兼容版本，并在本地提供它们。在每个系统上：像制作人那样安装（在 Windows 上使用 Windows PowerShell 5.1），检查版本、`doctor`、桥接及其分析工作进程，再次安装作为修复，运行 `kumi bridge --yes` 安装到一个临时的 Remote Scripts 文件夹，运行 `kumi update`、`kumi update --rollback` 和 `kumi uninstall`。过渡作业用 Kumi 1.7.5 和新的发行包运行迁移测试。在 `v*` 标签上，`publish` 随后把发行包附加到发布版本上。 |

要合并到 `main`，`Required CI` 必须通过。Installer 不是必需的。其余规则见[发布与分发](DISTRIBUTION_POLICY.md#合并门禁)。

## 通过意味着什么

通过表明代码的行为与其测试所描述的一致，各个包能在 macOS、Linux 和 Windows 上安装和运行，安装程序能在 GitHub 的运行器上工作。它并不表明 Remote Script 能在你的 Live 中加载、Live 的 API 与假对象的结构一致、任何东西听起来如何，或者某个终端或屏幕阅读器能与 Kumi 配合使用。上面那些需要主动运行的检查，以及[实现状态](IMPLEMENTATION_STATUS.md#证据)中的记录，覆盖的是真实的 Live。

## 编写测试

对于每个新的协议方法或对 Live 的修改，请添加一个证明它能工作的测试，并添加测试证明它会拒绝应当拒绝的情况：过期的引用、修订号和纪元（epoch）、过期的确认、重复使用的幂等键、超时、发送前和发送后的取消、断开连接、丢失的确认应答、部分修改、失败的补偿、期间在 Live 中做出的修改，以及撤销。在测试声称的内容中把假 Live、模拟器和真实 Live 区分开（`fake-live`、`simulator` 和 `real-live` 来源）。保持测试夹具小巧且不含隐私数据，绝不让测试触及真实的 Live 文件夹或 `~/.kumi`。
