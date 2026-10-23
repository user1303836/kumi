# 测试

[English](../en/TESTING.md) · 简体中文 · [日本語](../ja/TESTING.md)

如何运行仓库各部分的测试、它们需要什么，以及 CI 运行什么。所有常规测试都不需要 Live，也不需要登录。

## 快速开始

在仓库根目录运行：

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # 只需一次：部分 Rust 测试运行的官方 SDK
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
npm ci --prefix daws/ableton/live-extension
npm test --prefix daws/ableton/live-extension
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

它们需要：

| 需要 | 用于 |
| --- | --- |
| Rust 和 Cargo | 各个 crate 和迁移测试 |
| cargo-nextest（`cargo install cargo-nextest --locked`） | Rust 测试，每个测试在单独的进程中运行 |
| PATH 上的 Node.js（CI 使用 24） | 部分 Rust 测试、Live 扩展的测试和迁移测试 |
| PATH 上的 Python 3.11 或更高版本（`python3`，在 Windows 上为 `python.exe`） | Remote Script 测试、发布脚本的测试，以及会运行 Python 的 Rust 测试 |
| 在 `vendor/` 中本地提供的 Extensions SDK | 构建 Live 扩展或对其做类型检查（它的测试不需要） |

在 Windows 上，有几个测试会创建符号链接，这需要开发人员模式或管理员账户。没有这项权限时，其中一些会跳过，少数会以 `EPERM` 失败；CI 的 Windows 运行器具有这项权限。

## Kumi 与桥接

在仓库根目录运行。

| 命令 | 作用 |
| --- | --- |
| `sh scripts/test-isolated.sh`（Windows：`./scripts/test-isolated.ps1`） | 在独立的主目录中用 cargo-nextest 运行全部测试，每个测试一个进程；参数会传给 `cargo nextest run`（`KUMI_NEXTEST=0`：改用 `cargo test`，一个 crate 的测试共用一个进程） |
| `sh scripts/test-isolated.sh -p kumi-runtime -E 'test(/^hands_transport::/)'` | 只运行一个 crate 的一个测试文件：一个 crate 的测试是一个二进制（`tests/it/`，每个文件一个模块） |
| `cargo fmt --all --check` | 按 CI 的方式检查格式 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lint 检查；目前在 CI 中仅供参考 |
| `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref` | 跨 await 持有的 `RefCell` 借用；会使 Lint CI 任务失败 |
| `sh scripts/test-isolated.sh --profile ci` | 用 cargo-nextest 运行 `Rust / Linux` 各部分运行的测试（单个部分的用 `-p <crate>` 或 `--exclude <crate>`；`--profile ci-os`：macOS 和 Windows 运行的测试；`--profile extended`：全部测试） |
| `python3 scripts/dev.py check` | 针对本分支相对 `origin/main` 的改动（用 `--base` 指定其他起点）运行 Required CI 的检查：改动的空白、每个文件都属于某个 crate 或区域，然后对受影响的 crate 运行 `cargo fmt --check`、`RefCell` 的 lint 和它们的 `ci` 测试（用 cargo-nextest），再在有改动时运行 Remote Script、Live 扩展和 Renoise 工具的测试，以及发布脚本的测试。`--dry-run` 只说明会运行什么 |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | 桥接的性能预算；见[开发者指南](DEVELOPER_GUIDE.md#构建测试与测量) |

隔离运行器为测试提供独立的主目录：`HOME`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`、`XDG_CONFIG_HOME` 和 `KUMI_HOME` 都指向一个全新的临时文件夹，`KUMI_REMOTE_SCRIPTS_DIR` 和 `KUMI_LIVE_EXTENSIONS_DIR` 则被移除，因此任何测试都无法触及你的 Live 文件夹或 `~/.kumi`。

有些测试会在 Kumi 自己的客户端和提供方旁边运行官方的 MCP 和模型 SDK；在运行 `npm ci --prefix crates/kumi-runtime/tests/support` 之前，这些测试会失败。响度和真峰值测试会把分析结果与 FFmpeg 的 `ebur128` 对生成音频得出的结果进行比对。

`testdata/oracles/<crate>/` 中的 JSON oracle 文件（由该 crate 的测试通过 `kumi_testkit::oracle` 读取）是从 TypeScript 实现记录下来的基准文件（golden file）；该实现以及生成这些文件的脚本保留在 git 标签 `v1.7.6` 上，当前代码树中没有任何东西能重新生成它们。精简工作树（`python3 scripts/dev.py worktree new <名称> --lean`）不包含只有 Extended 测试才读取的那些（列在 `testdata/oracles/extended-only.txt` 中）。有意改变行为之后，请根据原生输出改写受影响的 oracle 条目：失败的断言会打印实际得到的结果；对于 SHA-256 条目，写入该输出的哈希。检查差异，并在提交信息中说明基准文件有改动及其原因。宿主测试固定使用记录基准文件时的桥接版本（`ORACLE_VERSION`），所以提升桥接版本号不会改变任何基准文件。

## Remote Script

在仓库根目录运行：

```sh
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
python3 -m compileall -q daws/ableton/remote-script/AbletonMcpBridge
```

这些测试针对假 Live 对象运行 Remote Script，覆盖：认证、顺序控制、主线程队列、注册表及其哈希、探查、事务、捕获与实时安全，以及可选的 Willington 提供方。

## Kumi 的 Live 扩展

`daws/ableton/live-extension` 有自己的 `package.json`。运行 `npm ci --prefix daws/ableton/live-extension` 之后，`npm test --prefix daws/ableton/live-extension` 会针对假 Live 加载已提交的 `dist/extension.js`，并对照记录的 sha256 检查它。在该目录中构建（`npm run build`）和类型检查（`npm run typecheck`）都需要 `vendor/` 中的 Extensions SDK；没有 SDK 时，已提交的构建保持原样。重新构建之后，请把 `dist/extension.js` 连同它的 `.sha256` 一起提交。

## 发布脚本

`python3 -m unittest discover -s scripts/tests -p 'test_*release.py'` 运行打包测试和迁移测试。打包测试检查发行包的内容和版本，以及打包的指南中每个链接都能解析。迁移测试针对原生发行包运行最后一个 JavaScript 版本的更新器。缺少以下变量时，需要它们的测试会跳过：

| 变量 | 指向什么 |
| --- | --- |
| `KUMI_LEGACY_APP` | 解包后的 Kumi 1.7.5：`python3 scripts/fetch-legacy-release.py [folder]` 会下载它、检查其 SHA-256、解包并打印所在文件夹 |
| `KUMI_NATIVE_RELEASES` | 存放已构建发布产物的文件夹（见[发布](DEVELOPER_GUIDE.md#发布)） |

`KUMI_LEGACY_APP` 也用于桥接自己的迁移测试 `daws/ableton/ableton-mcp-server/tests/it/lifecycle_migration.rs`。缺少该变量时，这个测试不会跳过，而是下载已发布的 Kumi 1.7.5 发行包并检查其 SHA-256。设置该变量后，只要发行包已下载过一次，它就可以离线运行。

## 需要 Live 或模型的检查

这些检查需要主动运行。它们会改动真实的东西或消耗真实的 token，所以 CI 不运行它们。

| 命令（在根目录运行） | 需要 | 作用 |
| --- | --- | --- |
| `cargo run --release -p kumi --example accept_live -- --set "<Set>"` | 打开了某个工程的一次性副本的 Live，以及事先构建好的桥接：`cargo build --release -p ableton-mcp-server --bins`（调试运行时，去掉 `--release` 即可） | 做出 Kumi 能做的每一类修改，用 Kumi 的撤销逐一撤销，播放、并轨、聆听和观看，并测量读取大型工程的耗时。不使用模型。 |
| `cargo run --release -p kumi --example time_rebuild` | 打开了一个 Set 的 Live；桥接，与 `accept_live` 一样先构建 | 不用模型，测量照着教程重建一遍在 Live 中要花多少时间：用一个计划建一条带 Drift 和四个效果器的 MIDI 轨道，读取每个设备的参数，按名称设置其中 30 个，以及 Kumi 查看 Set 的耗时，每项都附上它用了多少次桥接请求。它会添加一条轨道，并在最后删除。 |
| `cargo run --release -p kumi --example eval_changes [-- <part of a case name>]` | 你的登录和模型 | 检验模型如何使用 Kumi 的工具，针对一个合成桥接进行，它带有从原生工具目录读取的真实桥接工具 schema。从不触及 Live。每个用例给出所用时间、其中工具所占的时间，以及调用模型的次数；`EVAL_EFFORT` 设置模型的推理强度，`EVAL_TRACE=1` 逐一打印每次调用。`EVAL_TUTORIAL=1` 会加入一个来自 YouTube 的真实 16 分钟教程（需要联网）。`EVAL_MEASURE=1` 无需登录或模型，按字节打印每个请求都携带的内容：指令和每个工具的定义（`EVAL_MEASURE=tools` 还会打印定义本身）。 |
| `cargo run --release -p kumi --example probe_inference` | 你的登录 | 用一个无害的工具发送一次经认证的请求。从不触及 Live。 |
| `cargo run --release -p kumi --example probe_cache` | 你的登录和模型 | 像恢复对话时那样重建内核后，对话是否仍能用上服务商的提示缓存：每个简短回合打印输入词元和缓存词元，分别以同一对话、另一对话和不指定对话重建。从不触及 Live。 |

合成桥接中的 Operator、Saturator 和 EQ Eight 带有 Live 12.4 给它们的全部参数（从 Live 读入 `crates/kumi/examples/fixtures/eval_changes/live-devices.json`），并像 Live 一样运行 Kumi 自己设置参数的脚本。

## 文档

编辑文档之后，运行打包测试：

```sh
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

它们检查桥接随附的指南中的每个链接：桥接的 README，以及 `scripts/build-native-release.py` 的 `DOCUMENTS` 中列出的十四篇 `docs/en` 页面。没有任何检查会核对三种语言是否一致，请手动保持同步。发布脚本的测试（`test_release.py`）会检查 REAPER 扩展和 Renoise 工具在每种语言的文档（`KUMI_GUIDE.md`）中各有一行 ships-with 说明，并且写的是这个检出所带的版本。

## CI

每个拉取请求都会运行下面这些工作流，其中 CI、Willington files、Installer 和 Network 在每次推送到 `main` 时也会运行：

| 工作流 | 作业 | 运行内容 |
| --- | --- | --- |
| **CI** | `Rust / Linux`、`Rust / Linux (Live's bridge)`、`Rust / Linux (DAW bridges)` | 通过隔离运行器运行 `.config/nextest.toml` 的 `ci` 集合，按 crate 分成三个部分同时运行（Kumi 及其运行时；`ableton-mcp-server`；`daw-bridge`、`daw-mcp-server` 和 `reaper-kumi`），所以没有哪个运行器要构建全部测试二进制：除 Extended 工作流运行的长测试（其中包括 REAPER 和 Renoise 的完整流程模拟，配置文件中列出并说明原因）之外的全部测试。在拉取请求上，每个部分只测试改动影响到的本部分 crate（由 `scripts/dev.py` 选出，与 `dev.py check` 相同），没有受影响 crate 的部分会跳过。运行超过 30 秒的测试会按名称失败。然后运行 `git diff --check`。每个 Rust 作业（包括 Extended 的）都会把它选中的测试列表保存为构件（`nextest-list-*`），以便比较不同运行的测试 |
| | `Rust / macOS`、`Rust / Windows` | `ci-os` 集合：因平台而异的测试（安装、启动和更新；终端、stdin 和 Windows 控制台；Hands；路径、文件和子进程；存储），也只构建这些测试 |
| | `Rust / Lint`（Ubuntu） | `cargo fmt --check`、示例的构建和 Clippy（仅供参考，但跨 await 持有 `RefCell` 借用会使该任务失败） |
| | `Python Remote Script / ubuntu-24.04`、`macos-15`、`windows-2025`（Python 3.11） | Remote Script 的测试；编译该包 |
| | `Live extension`（Ubuntu，Node 24） | 针对已提交的构建运行扩展的测试 |
| | `Release scripts`（Ubuntu） | 检查本次修改的空白字符，然后运行打包测试、发布脚本的测试和 `scripts/check-installers.sh`（install.sh 是 POSIX sh；install.ps1 能被解析且只含 ASCII）。推送到 `main` 时，还会检查 `main` 的 raw 安装程序和最新版本中的安装程序 |
| | `REAPER extension loads / macos-15`、`ubuntu-22.04`、`windows-2025` | 按发布时的样子检查 REAPER 扩展：发布构建和平台自己的 strip，发布脚本对发行包可携带内容的检查（REAPER 的入口点；macOS 上的签名和自身名称；Windows 上静态链接的 C 运行库；Linux 上不需要比 Kumi 更新的 glibc），用系统的加载器加载，以及 REAPER 经由 C++ 垫片（shim）的调用 |
| | `Renoise tool`（Ubuntu） | 在固定版本的 LuaJIT 上针对 Renoise API 的模拟运行工具的 Lua 测试，以及对照 Renoise API 6.2 定义的 `lua-language-server --check` |
| | `Required CI` | 只有以上全部通过时才通过 |
| **Extended** | `Extended / Linux (1/3)` 至 `(3/3)`、`Extended / macOS (1/2)`、`(2/2)`、`Extended / Windows (1/3)` 至 `(3/3)`、`Extended / documentation tests` | 在装好官方 SDK 的情况下运行全部测试（`extended` 集合）：`ci` 省去的内容（录制源的预言测试，长时间的聆听、曲式和渲染模拟，使用 ffmpeg 的视频测试，基准测试，与官方 MCP 和模型 SDK 并排运行的互操作测试，从 Kumi 1.7.5 的迁移，计时构件的长时间扫描）以及每个平台的全部测试；然后是文档测试。每周、手动以及在带有 `full-tests` 标签的拉取请求上运行，不阻塞任何合并 |
| | `Extended / REAPER SDK, newest header` | 用 Cockos 发布的最新 SDK 头文件构建 REAPER 扩展，检查生成的函数表，并列出改用它们会新增的函数 |
| **Network** | `Network / What changed`，然后是 `Network / Linux`、`macOS`、`Windows` | Kumi 的 HTTP 客户端，使用装进各系统自身信任存储的证书颁发机构，并经过系统自身设置中的代理（`network.rs` 中被忽略的测试）。各系统的作业只在修改客户端构建方式的变更（`scripts/areas.toml` 中的 `network` 区域）时运行 |
| **Willington files** | `Willington files`（Ubuntu） | 只有仓库所有者从本仓库 `willington/` 分支发出的拉取请求才能修改 `vendor/willington/`，而这样的拉取请求不修改其他任何内容。运行的是 `main` 上的这项检查，它读取拉取请求的文件列表，从不读取其代码 |
| **Installer** | `Native bundle / <target>`、`Aggregate native and existing-installer releases`，然后是 `Install / <system>` 和 `Existing installer transition / <system>`；`Installer edges / <system>`（三个）不需要发行包 | 每次推送到 `main`，以及修改安装的拉取请求，会为 x64 的 Linux 和 Windows、Apple Silicon 的 macOS 构建发行包，并在每个系统上按下文安装、检查和卸载。标签、手动运行和带有 `full-tests` 标签的拉取请求运行全部内容：为 macOS、Linux 和 Windows 的 Intel 与 ARM 构建原生发行包（Mac 发行包带有 Kumi 在 Mac 上使用 Live 菜单的辅助程序，通用、临时签名；Intel 的发行包在 Apple Silicon 上交叉构建），再构建现有安装用来更新的兼容版本，并连同安装程序一起，按 GitHub 提供发布文件的方式在本地提供它们。在每个系统上：像制作人那样用一行安装命令从这里安装（在 Windows 上使用 Windows PowerShell 5.1，`KUMI_HOME` 位于名称含非 ASCII 字符的文件夹中），检查版本、`doctor`、桥接及其分析工作进程，再次安装作为修复，运行 `kumi bridge --yes` 安装到一个临时的 Remote Scripts 文件夹，把 REAPER 的扩展和 Renoise 的工具装进各放有一个制作人自己文件的临时 DAW 文件夹（`kumi --daw reaper|renoise bridge --yes`），检查每个都是发行包中的副本并带有参考文件、回执和保留的版本，用 REAPER 的入口点加载扩展，并启动 Kumi 为它们准备的宿主程序，然后运行 `kumi update`（两个桥接保持原样）、两次 `kumi update --rollback`（两者都回到之前 Kumi 的副本，再回来）和 `kumi uninstall`（两者都移除，保留制作人改动过的工具副本，制作人自己的文件不动）。随后每个系统上的一个安装（macOS arm64、Linux x64、Windows x64）在含空格和非 ASCII 字符的主目录中：用本次构建的安装程序安装 GitHub 的最新版本，再用 `KUMI_VERSION` 安装本次构建之前的那个版本；装入该版本的桥接；更新到本次构建并回滚两次，每次都检查 Kumi 和桥接（同时装有 REAPER 和 Renoise 的桥接，那个版本两者都不认识，会原样留下）；最后卸载。PATH 那一行在 macOS 上写入 zsh 的 `ZDOTDIR`，在 Linux 上写入 fish 的 `conf.d`，由该 shell 找到 `kumi`，卸载时被移除；Windows 在 PowerShell 7 中运行这些：本次构建的安装、桥接、回滚和卸载在名称含非 ASCII 字符的文件夹中的 `KUMI_HOME` 下进行，从之前那个版本的更新则在只含空格的主目录中进行（由该版本自己的更新程序解包）。过渡作业用 Kumi 1.7.5 和新的发行包运行迁移测试。edges 作业针对替代版本运行 install.sh 和 install.ps1：版本不存在、无应答、返回登录页面、校验和不匹配、Kumi 无法启动、Ctrl-C。推送 `v*` 标签时，在这些都通过之后，`publish` 把发行包和安装程序附加到发布版本上，并在说明由 `scripts/release.py` 撰写时发布该版本。 |

在拉取请求上，运行的内容更少：

- `Python Remote Script` 跳过 macOS。
- Rust 的构建缓存只从 `main` 保存：拉取请求恢复 `main` 的缓存，不保存任何缓存。
- 每个部分的作业只在它测试的内容改变时运行，由 `scripts/dev.py` 根据各 crate 和 `scripts/areas.toml` 判断（`python3 scripts/dev.py affected --base origin/main` 会说明一个分支运行什么）：只改 Renoise 工具的 Lua 时 Rust 的作业会跳过；`REAPER extension loads` 在扩展的 crate、它构建时用到的内容（`daw-bridge`、锁文件）和发布脚本改变时运行；`Renoise tool` 在 `daws/renoise` 改变时运行。`Release scripts` 会检查每个被跟踪的文件都属于某个 crate 或区域。
- 只有修改安装相关部分的变更，Installer 才会在每个系统的一台机器上构建并安装：安装程序、安装、更新、发布、Willington 的文件、Mac 辅助程序、Kumi 使用的 `tar` 和 PowerShell、下载方式（HTTP 客户端和网络设置）和路径的拼接、Kumi 把扩展放进 Live 的方式、REAPER 和 Renoise 桥接的构建与装入方式，以及 Installer 的工作流（不包括它们的测试）。其他拉取请求什么都不构建，合并后由 `main` 的运行来检查。全部六个、从 GitHub 版本的更新、edges 和迁移在 `full-tests` 标签下运行。
- 在拉取请求和推送到 `main` 时，Installer 的发行包用 `ci-release` 配置构建，它省去 release 配置的全程序优化，优化程度也更低。标签用 `release` 构建。

每次推送到 `main` 运行与修改安装的拉取请求相同的内容，每个标签运行 Installer 的全部内容。Extended 工作流每周运行全部测试。Installer 每晚会用 `release` 配置构建全部六个发行包（失败时会开一个 issue：“The nightly Installer build failed”），从而为下一个标签保持该配置的构建缓存。Caches 工作流每天删除 Rust 构建缓存中除每类最新一份以外的所有缓存，这样旧的锁文件世代就不会把拉取请求要恢复的缓存挤出仓库的 10 GB 空间。

要合并到 `main`，`Required CI` 和 `Willington files` 必须通过。Installer 不是必需的。其余规则见[发布与分发](DISTRIBUTION_POLICY.md#合并门禁)。

## 通过意味着什么

通过表明代码的行为与其测试所描述的一致，各个包能在 macOS、Linux 和 Windows 上安装和运行，安装程序能在 GitHub 的运行器上工作。它并不表明 Remote Script 能在你的 Live 中加载、Live 的 API 与假对象的结构一致、任何东西听起来如何，或者某个终端或屏幕阅读器能与 Kumi 配合使用。上面那些需要主动运行的检查，以及[实现状态](IMPLEMENTATION_STATUS.md#证据)中的记录，覆盖的是真实的 Live。

## 编写测试

对于每个新的协议方法或对 Live 的修改，请添加一个证明它能工作的测试，并添加测试证明它会拒绝应当拒绝的情况：过期的引用、修订号和纪元（epoch）、过期的确认、重复使用的幂等键、超时、发送前和发送后的取消、断开连接、丢失的确认应答、部分修改、失败的补偿、期间在 Live 中做出的修改，以及撤销。在测试声称的内容中把假 Live、模拟器和真实 Live 区分开（`fake-live`、`simulator` 和 `real-live` 来源）。保持测试夹具小巧且不含隐私数据，绝不让测试触及真实的 Live 文件夹或 `~/.kumi`。
