# 开发者指南

[English](../en/DEVELOPER_GUIDE.md) · 简体中文 · [日本語](../ja/DEVELOPER_GUIDE.md)

本仓库的各部分如何组合在一起、如何开发每个部分，以及如何发布。[测试](TESTING.md)列出了所有测试命令以及 CI 运行的内容。

## 布局

| 文件夹 | 内容 |
| --- | --- |
| `crates/kumi` | 原生的 `kumi` 命令和终端应用（`src/tui/`），每个 DAW 的设置、doctor 和更新步骤（`src/daw/`），以及针对模型、真实 Live 或真实 Logic Pro、需要主动运行的检查（`examples/`） |
| `crates/kumi-runtime` | 代理循环、模型提供方、登录、会话、素材库、各 DAW 的集成（Live 的，以及经由 Kumi 主机的 REAPER、Renoise 和 Logic Pro 的）、媒体工具和 MCP 客户端 |
| `crates/kumi-common` | 共享的运行时工具，以及与原实现兼容的值处理 |
| `crates/kumi-store` | Kumi 的数据库：它在会话之间保存的内容，存于 SQLite 中 |
| `crates/kumi-genexpr` | 用于测试 Kumi 的 Max for Live 构件的 GenExpr 解释器 |
| `crates/ableton-mcp-server` | Live 的原生 MCP 桥接、分析工作进程、生命周期、设置、迁移和诊断 |
| `remote-script` | 桥接的 Remote Script，运行在 Live 内部（`ableton_mcp_remote_script.py`、`AbletonMcpBridge/` 入口、Python 测试） |
| `apps/live-extension` | Kumi 的 Live 扩展，适用于 Live 12.4 及更高版本，基于 Live 的 Extensions SDK（TypeScript，有自己的 `package.json`） |
| `protocol` | `ableton-live-v1.operations.json`，桥接与 Remote Script 共用的操作注册表 |
| `scripts` | 发布构建器、迁移打包、Mac 辅助程序的构建、隔离测试运行器，以及 `npm run kumi` 和 `npm run setup` 背后的 `native-kumi.mjs` |
| `crates/daw-mcp-server` | Kumi 为 REAPER、Renoise 和 Logic Pro 准备的宿主程序（`daw-mcp-server --daw reaper\|renoise\|logic`）：基于 stdio 的 MCP、修改的日志与撤销、聆听任务、离开期间的变化，以及测试所针对的模拟 REAPER、Renoise 和 Logic Pro；其他 MCP 客户端也能使用 |
| `crates/reaper-kumi` | Kumi 的 REAPER 扩展（`reaper_kumi`），用 `sdk/` 中附带的 REAPER SDK 头文件构建 |
| `crates/daw-bridge` | REAPER、Renoise 与 Logic Pro 桥接共用的部分：回环通信（`kumi-daw/1`）、它的握手与认证、发现、查找 DAW、doctor 的检查，以及桥接文件的生命周期 |
| `daws/renoise` | Kumi 的 Renoise 工具（`com.kumi.Bridge.xrnx`，Lua）及其针对 Renoise API 模拟的测试 |
| `crates/logic-kumi` | Kumi 的 Logic Pro 桥接（在 Logic Pro 旁运行的程序）及其辅助功能辅助程序（`helper/KumiLogicHands.swift`，由 `scripts/build-logic-hands.py` 构建）；见 [Logic 契约](LOGIC_CONTRACT.md) |
| `install.sh`、`install.ps1` | 安装程序 |

这些 crate 共用根目录的 Cargo 工作区；没有 npm 工作区。Live 的桥接没有 Kumi 也能工作。这些 crate 移植自的 TypeScript 实现保留在 git 标签 `v1.7.6` 上。

## 各部分如何通信

```text
kumi (crates/kumi, crates/kumi-runtime)
  │  MCP over stdio: Kumi starts the bridge as a child process
  ▼
bridge (crates/ableton-mcp-server)
  │  ableton-loopback/v1: authenticated TCP on 127.0.0.1
  ├──► Remote Script inside Live (remote-script/)
  │  local channel to the Extension Host
  └──► Kumi's Live extension (apps/live-extension), Live 12.4+

kumi (crates/kumi, crates/kumi-runtime)
  │  MCP over stdio: Kumi starts its host for REAPER or Renoise
  ▼
host (crates/daw-mcp-server --daw reaper|renoise)
  │  kumi-daw/1: authenticated TCP on 127.0.0.1 (crates/daw-bridge)
  ├──► Kumi's REAPER extension (crates/reaper-kumi), in REAPER's UserPlugins
  └──► Kumi's Renoise tool (daws/renoise), in Renoise's Scripts/Tools
```

- Kumi 以 `full` 部署策略启动桥接，并附带一个只包含它所用工具的允许列表（`ABLETON_MCP_TOOL_ALLOW`）。模型直接调用少数几个桥接读取工具（`crates/kumi-runtime/src/mcp/allowed_tools.rs` 中的 `MODEL_TOOLS`）；其余的由 Kumi 自己的工具调用。
- 桥接的路由器（`crates/ableton-mcp-server/src/bridge/router.rs`）把每个操作发送给 Remote Script，或者在只有扩展具备该操作时发送给扩展。当 Live 的 Developer Mode 让 Live 不启动扩展时，桥接可以自己启动 Live 的 Extension Host。
- `kumi bridge` 通过桥接的生命周期命令把桥接安装到 Live 中，之后通过 `Remote Scripts/AbletonMcpBridge/bridge-reference.json` 找到它。

Logic Pro 没有脚本接口，也不加载任何东西，所以它的桥接是 Kumi 在它旁边运行的一个程序：

```text
kumi (crates/kumi, crates/kumi-runtime)
  │  MCP over stdio: Kumi starts its host as a child process
  ▼
host (crates/daw-mcp-server, `daw-mcp-server --daw logic`)
  │  kumi-daw/1: authenticated TCP on 127.0.0.1 (9790 to 9799); the host starts the bridge
  ▼
bridge (crates/logic-kumi, `logic-kumi`, from Kumi's own folder)
  │  JSON lines on stdin and stdout: the bridge runs its helper as a child
  ▼
helper (crates/logic-kumi/helper, `kumi-logic-hands-<hash>`)
  │  Accessibility, key events and clicks, virtual CoreMIDI ports (Mackie Control), a Core Audio process tap
  ▼
Logic Pro
```

- 对于桥接由 Kumi 自己提供的每个 DAW，主机都是同一个二进制文件（`--daw logic|reaper|renoise`）。对 Logic Pro，它用 `--kumi-home` 启动已安装的桥接，通过 `~/.kumi/bridges/logic/discovery/` 中桥接的发现文件找到它，证明双方都知道共享的密钥，并在 Kumi 的会话结束时停止它。Kumi 为 Logic Pro 准备的修改集（`crates/kumi-runtime/src/integrations/logic_changes.rs`）定下各个工具的名称；主机的修改日志（`crates/daw-mcp-server/src/changes.rs`）通过桥接预览、应用和撤销每项修改。
- `kumi --daw logic bridge` 通过 daw-bridge 的生命周期把桥接安装到 Kumi 自己的文件夹中，来源是发布版本中 Kumi 旁边的副本（`daws/logic/logic-kumi/`）、`KUMI_LOGIC_BRIDGE_SOURCE`，或者，在 Mac 上的源码副本中，是暂存在 `target/logic-bridge/logic-kumi/` 中的该源码副本的构建（辅助程序不存在时由它的脚本构建）。

## 环境搭建

需要 Rust、Cargo、Python 3.11 或更高版本，以及 git：

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --
cargo run --release -p kumi -- bridge --allow-dirty   # 需先关闭 Live
```

`npm run setup` 和 `npm run kumi -- ...` 仍然可用：装有 Cargo 时，它们用 Cargo 构建当前检出的代码；否则交给对应版本的已发布原生版。这些命令、Live 扩展和部分测试需要 Node.js，原生应用本身不需要。

检出与已安装的 Kumi 共用 `~/.kumi`（设置、登录信息、对话、桥接的状态）。`--allow-dirty` 允许 `kumi bridge` 从带有未提交修改的检出中安装桥接。在 Windows 上，如果没有开启开发者模式、也没有使用提升权限的 shell，创建符号链接的测试会跳过或失败；CI 的运行器可以创建符号链接。

## 构建、测试与测量

在仓库根目录运行原生检查。隔离运行器会在临时主目录中用 cargo-nextest 运行全部测试，每个测试一个进程：

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # 只需一次：部分测试运行的官方 SDK
cargo build --locked --workspace --all-targets
sh scripts/test-isolated.sh                 # PowerShell: ./scripts/test-isolated.ps1
sh scripts/test-isolated.sh -p kumi-runtime -E 'test(/^hands_transport::/)'
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

性能门槛要用优化后的二进制文件运行，并避开覆盖率统计或其他大型构建。启动基准测试之前，先构建同目录中的分析工作进程：

```sh
cargo build --locked --release -p ableton-mcp-server --bins
cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark
```

基准程序输出 JSON 测量结果，超出预算时以非零状态退出。它的内存列统计的是被跟踪的 Rust 分配量，并不是分开的 V8 堆、external 和 ArrayBuffer 测量值。调试构建的计时不能作为发布版性能的依据。

其他测试，以及针对模型和真实 Live、需要主动运行的检查，见[测试](TESTING.md)。

## 开发 Kumi

**一个修改类型**（一种拥有自己的 HISTORY 行和撤销的修改）是 `CHANGES`（`crates/kumi-runtime/src/integrations/ableton/changes.rs`）中的一个条目。元数据来自 `assets/changes.json`；摘要位于 `changes/summaries.rs` 和 `changes/more_summaries.rs`。每个条目写明 Kumi 的工具、桥接的 preview 和 apply、一个 family（HISTORY 和 NOW 所绘制的一组固定图形之一）、给模型的描述，以及通俗易懂的摘要。如果较旧的桥接会拒绝它，就给它加上 `since`（它适用的第一个桥接版本，定义在 `bridge_version.rs` 中）；如果 Live 没有办法撤回它，就加上 `permanent`。测试会检查每个修改类型：工具唯一、只凭最简的预览也能得出标题、描述绝不要求模型确认，以及所用的桥接工具仅限宿主使用。在提供它之前，先在真实 Live 上运行它及其撤销（`accept_live` 示例）。

**一个动作**（不是对工程的修改、没有可撤销的内容，比如播放）放在 `actions.rs` 的 `ACTIONS` 中，元数据位于 `assets/actions.json`。

修改评估（`eval_changes` 示例）从桥接的工具目录读取工具 schema，因此工具变化时无需重新生成任何东西。

终端应用的设计和基础见[命令、按键与界面](KUMI_TUI.md#设计说明)。

## 开发桥接

下面的路径都相对于 `crates/ableton-mcp-server`。

| 路径 | 说明 |
| --- | --- |
| `src/host.rs`、`src/host/` | MCP 分发、严格的工具模式、事务、撤销与恢复 |
| `src/tool_catalog.rs` | 唯一的工具目录：模式、注解、每个工具所需的能力，以及它的部署策略类别 |
| `src/live.rs`、`src/registry.rs` | Live 类型与适配器；注册表及其哈希的加载与验证 |
| `src/bridge/` | 经过认证的回环客户端（`remote_adapter.rs`）、路由器，以及扩展的通道、启动器和文件夹 |
| `src/transactions/` | 批处理、设备状态、Session MIDI 和发现辅助工具 |
| `src/mcp_protocol.rs`、`src/stdio.rs` | 两个协议版本的 MCP 传输处理 |
| `src/analysis*.rs`、`src/audio_*.rs`、`src/reference_analysis.rs` | 在隔离的工作进程中进行音频分析 |
| `src/delivery*.rs`、`src/lifecycle*.rs`、`src/setup.rs`、`src/migrate.rs`、`src/diagnostics.rs` | 配置、密钥、安装、升级、回滚与诊断 |
| `src/als.rs`、`src/project*.rs`、`src/library_search.rs` | 已保存的工程、工程快照与差异、Live 的库数据库 |
| `src/follow_actions.rs` | 可选的 [Willington](WILLINGTON_INTEGRATION.md) Follow Actions |

**契约规则。**

- 传输协议是 `ableton-loopback/v1`：规范 JSON（键排序、负零归一化）、请求和响应上的 HMAC-SHA256、有界的帧和集合、序列号，以及每次 Remote Script 启动时都会改变的 epoch。详情见 `remote-script/README.md`。
- `protocol/` 中的注册表是唯一的操作列表。宿主和 Remote Script 各自计算它的哈希，两者必须一致，否则 Live 永远不会连接；有一个宿主测试会运行 Remote Script 的哈希计算，确保两者相等。绝不要把操作名称或哈希复制到其他源文件中。
- 修改通过各有专门用途的操作进行，每个操作都有预览、应用和撤销。唯一的例外是 `python.run`（`live_run_python`），它在 Live 的主线程上运行 Python，唯一的回退方式是 Live 的撤销；它有自己的策略类别 `python`，只有 `full` 配置文件允许使用。
- 桥接内部的 `get(ref)` 是基于固定行的有界序列化器，而不是 Live 对象模型的通用读取器；MCP 读取保持各有专门用途。
- Remote Script 在 Live 的主线程上完成它与 Live 打交道的全部工作：它在 Live 的显示刷新周期内（两次刷新之间借助 Live 自己的计时器）、在限定的时间预算内自己处理套接字。它仅有的其他线程用于写入诊断文件和接收实时 UDP。新的 epoch 会使之前的所有引用和游标失效。Remote Script 无法识别的 Live 结构会被报告为不可用，绝不伪造。
- stdout 只承载 MCP 协议。诊断信息输出到 stderr，且不含请求数据。
- 基于进程的操作使用 `AsyncLiveAdapter`；模拟器仍保留同步方法。兼容性改动要针对这两条路径测试。
- 测试绝不需要正在运行的 Live、某个设备、某台特定的机器或仅限本地的材料。每个新操作都要有测试，包括错误输入和恢复。

**MCP 版本。** 桥接同时支持 `2025-11-25`（先 initialize，再发请求）和 `2026-07-28`（每个请求的 `params._meta` 带有协议版本和客户端能力，另有 `server/discover`）；一个进程只使用其中一种。未知版本返回 `-32022`；错误的元数据返回 `-32602`。新版本的结果带有 `resultType: "complete"`，并且也会在 `structuredContent` 中返回 JSON。新版本没有推送：`live_subscribe` 只在旧版本中可用，新版本的客户端需要轮询。客户端元数据绝不会授予对 Live 的访问权限。请见[规范](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)。

## 开发 Logic Pro 桥接

[Logic 契约](LOGIC_CONTRACT.md)是参考：桥接在 `kumi-daw/1` 上应答什么，每个应答和每项修改经过 macOS 的哪个通道，以及辅助程序的协议（`crates/logic-kumi/helper/README.md` 列出了它的每个操作）。`docs/evidence/logic-*.md` 中的实验记录和关口（gate）记录说明了在真实 Logic Pro 上测得了什么。

- **没有测试需要 Logic Pro。** 桥接的测试重放从真实 Logic Pro 录下的场景（`crates/logic-kumi/tests/fixtures/`，用 `record_fixtures` 和 `record_changes` 示例录制，它们把主文件夹保存为 `/Users/me`）；主机的测试在桥接录下的读取（`crates/daw-mcp-server/tests/vectors/logic-reads.json`）之上运行模拟的 Logic Pro（`crates/daw-mcp-server/src/simulator/logic.rs`）；运行时和应用的测试则在进程内经由主机运行 Kumi。
- **真实的 Logic Pro** 交给需要主动运行的 `accept_logic` 示例，每个关口都运行过它（[测试](TESTING.md#需要-live-或模型的检查)）。
- **Logic Pro 的名称**只留在 `crates/logic-kumi`、`daw-bridge` 和 `daw-mcp-server` 中与 Logic Pro 有关的部分、运行时中的 `integrations/logic*.rs`、`crates/kumi/src/daw/own.rs` 以及测试中：运行时的共用代码只看到 DAW 的配置档（profile），`daw_neutral_guard.rs` 和假 DAW 测试则让 Logic Pro 会话所说的话中不出现 Live 的用语。
- **辅助程序**以其源代码的哈希命名（`kumi-logic-hands-<hash>`），发布版本带的正是当前源代码的构建：修改 `KumiLogicHands.swift` 之后，用 `python3 scripts/build-logic-hands.py` 重新构建它。

## 开发 Live 扩展

`apps/live-extension` 使用 TypeScript 编写，用 esbuild 打包，有自己的 `package.json`。它基于 Live 的 Extensions SDK 构建，而这个 SDK 的许可证禁止再分发，所以它不在仓库中：请把一份副本放到仓库根目录的 `vendor/ableton-extensions-sdk-1.0.0-beta.1/`（构建会读取其中的 `package 3/dist/index.cjs`）。在 `apps/live-extension` 中运行 `npm ci`，然后运行 `npm run build`，它会写出 `dist/extension.js` 及其 `.sha256`；两者都要提交。`npm run typecheck` 同样需要 SDK。没有 SDK 时，构建会停止，已提交的包保持不变；`npm test` 会根据校验和检查这个包。关于 Live 如何运行该扩展的测量数据，见[证据](../evidence/live-extension.md)。

## Willington 的文件

[Willington](WILLINGTON_INTEGRATION.md) 的仓库是私有的：Kumi 只带有它的运行时文件，放在 `vendor/willington/` 中，这个文件夹只在 Willington 更新（见下文）时改变。文件旁边还有 Willington 的许可声明（`LICENSE` 或 `LICENSE.md`；Kumi 的 MIT 许可证不涵盖这些文件）和 `release.json`：

```json
{"schema": "kumi-willington-vendor/v1", "version": "0.4.0", "commit": "<Willington 的 40 位提交>",
 "files": {"WillingtonRuntime/__init__.py": "<SHA-256>", "LICENSE": "<SHA-256>"}}
```

`files` 列出文件夹中除它自身以外的每个文件，文件名须是每个平台的检出都能容纳的：只用 ASCII 字母、数字、`.`、`_` 和 `-`，不能是 Windows 设备名或以点结尾，也不能有两个只差大小写的名字。当文件夹里有 `release.json` 没有列出的文件、SHA-256 不符的文件，或者不是 Willington 运行时文件的文件时，`scripts/build-native-release.py` 会拒绝发布：只有 `WillingtonRuntime`、`WillingtonBindings`、`WillingtonDeviceTools` 和 `WillingtonRackZones` 中的 `.py`、`.json`、`.md`、`.pyd` 和 `.dylib` 文件才符合，所以源代码、头文件、调试文件和字节码缓存永远不会被发布。`test_native_release.py` 在 CI 中对仓库里的这个文件夹做同样的检查，`.gitattributes` 让这个文件夹逐字节保持原样，并且不做空白检查。

发布会把这些文件放在桥接的 Remote Script 中，即 `AbletonMcpBridge/willington/`，原生库只带本平台的（Windows 上是 `.pyd`，macOS 上是 `.dylib`），总共最多 16 MiB；Linux 的安装包不带。桥接安装时会一并复制这个文件夹，并在 Python 文件旁边放一个 `__pycache__` 阻挡文件，使安装后的文件树保持安装回执所记录的样子。已安装的桥接中有两个文件属于制作人而不属于发布：`willington.json` 和跟随动作自检回执 `willington/WillingtonBindings/self-test.json`。它们不影响桥接的安装检查，安装时也会保留下来。只有当 `willington.json` 开启 Willington 后，桥接才会把这个文件夹加入 Python 的路径，并且排在安装在桥接旁边的副本之后。

**更新 Willington。** Willington 的 Bundle 工作流在每个目标平台上构建它的原生库，并把矩阵发行包保留为每次运行的产物。更新使用推送到 Willington `main` 时的一次运行：

1. 找到这次运行：`gh run list -R xonedsp/willington -w Bundle -b main -e push -s success`。
2. 在从 `main` 创建、名为 `willington/<任意名称>` 的分支上，运行 `python3 scripts/vendor-willington.py --run <run>`。它会检查这次运行是推送到 Willington `main` 后成功的 Bundle 运行，并且它的提交在 `main` 上；检查产物与 GitHub 记录的摘要一致、发行包与它的 SHA-256 一致；再取得那个提交上 Willington 的许可声明。只有新文件夹通过发布检查后，它才会替换原来的文件夹，然后打印提交、运行和产物的摘要。
3. 打开一个不修改其他任何内容的拉取请求，写上它打印的内容。`Willington files` 只让仓库所有者从本仓库 `willington/` 分支发出的拉取请求通过，Installer 会在全部六个平台上构建并安装。

审查者用 `python3 scripts/vendor-willington.py --check <run>` 检查一次更新：它从这次运行重新构建文件夹，并逐个文件比较。

这些库由 CI 构建，所以它们的哈希可能与在 Live 中验证过的不同。在发布这次更新的版本之前，请先在 Live 中检查它，并为它的库运行[跟随动作自检](WILLINGTON_INTEGRATION.md#跟随动作自检)。

## 发布

**提交**的标题用通俗的英文，说明对制作人来说改变了什么（“Kumi: talk to it while it works”）。桥接或 Remote Script 的修改要提升 `crates/ableton-mcp-server/Cargo.toml` 和 `Cargo.lock` 中的版本，标题以新版本开头（“Bridge 1.0.71: …”），并在 `CHANGELOG.md` 的 `## Unreleased` 下添加一个 `### Bridge x.y.z` 块。如果扩展变了，重新构建并提交它的包。工作在分支上进行，通过拉取请求合并到 `main`。

**一次 Kumi 发布**只需一条命令，可在任意源码副本中运行，`gh` 需以仓库管理员身份登录。在拉取请求合并后立即运行：

```sh
python3 scripts/release.py          # 试运行：显示版本、CHANGELOG 条目、发布说明和各项检查
python3 scripts/release.py --go     # 正式发布
```

- **发布内容**读取自上一个标签以来的 `main`：每个已合并拉取请求的 `Changelog:` 行，按原文使用（“none” 不添加任何内容）。如果拉取请求修改了桥接，而没有修改 Kumi 的 crate 或安装程序，它的行会放在桥接的标题下；同时修改两者的拉取请求，请把关于桥接的行写成 `Changelog (bridge):`。试运行会显示每一行放在哪里。
- **Logic Pro 桥接**在拉取请求修改了它运行的内容时，以同样的方式获得新版本：`crates/logic-kumi` 及其辅助程序，或它链接的 `daw-bridge`（测试、示例、Markdown 以及 `crates/logic-kumi/keys/` 中的快捷键命令工具除外）。只修改它、不修改其他任何随发布交付的内容的拉取请求，它的行放在它的标题 `### Logic Pro bridge x.y.z` 下；其他情况下，关于它的行要写成 `Changelog (logic):`。`--logic` 无论改了什么都给它一个新版本，`--no-logic` 则保持它的版本不变。它的版本在 `crates/logic-kumi/Cargo.toml` 中，发布时会提升一个补丁版本。
- **What's new**：Kumi 内置 `CHANGELOG.md`，更新后第一次启动时，把一个版本的列表项显示为 **What's new**（最多五条，从最新版本开始；其余见 `/changelog`）。段落（例如附带桥接和测试环境的说明）不会显示，所以每一行 `Changelog:` 就是用户在那里读到的一条：写他们会注意到的变化，重要的放在前面。
- **桥接**在拉取请求修改了 Live 加载的内容时获得新版本：`crates/ableton-mcp-server`、主机链接的 `crates/kumi-common`、`remote-script`、`protocol`、`apps/live-extension` 或 `vendor/willington`（测试和 Markdown 除外）。`Cargo.lock` 中主机的依赖变化时，试运行会提示，这时用 `--bridge` 给桥接一个新版本。
- **REAPER 的扩展和 Renoise 的工具**也按同样的方式获得新版本：拉取请求修改了该 DAW 加载的内容时（`crates/reaper-kumi` 和 `crates/daw-bridge`，或 `daws/renoise/com.kumi.Bridge.xrnx`）。`--reaper` 和 `--renoise` 无论如何都给它新版本，`--no-reaper` 和 `--no-renoise` 保持原版本；`--no-bridge` 只针对 Live。写成 `Changelog (reaper):` 或 `Changelog (renoise):` 的行会放到它的标题下。使用 `--go` 时，`crates/reaper-kumi/Cargo.toml` 和 `Cargo.lock`、工具的 `payload/version.lua` 和 `manifest.xml` 中的版本会前进，三个 `KUMI_GUIDE.md` 中各自的 ships-with 行也会前进。上一个版本没有的桥接按它自己的版本发布，它那一行已经写着本次版本。
- **版本**是下一个补丁版本；`--minor` 或 `--version X.Y.Z` 可以选择其他版本。
- **加上 `--go`**，它会在 `release/vX.Y.Z` 上提交 “Kumi X.Y.Z: the changelog, READMEs and versions”：根目录的 `package.json`、`crates/kumi-runtime/src/version.rs`、`kumi`、`kumi-common` 和 `kumi-runtime` 的 Cargo 清单以及 `Cargo.lock` 中的版本（打包测试会确保这些相等），桥接变化时还有桥接的版本；三个 README 中当前状态（Status）的那一行；三个 `KUMI_CHANGES.md` 中“桥接版本”（Bridge versions）下说明随附哪个桥接的那一行；以及 `CHANGELOG.md` 中的 `## X.Y.Z — date` 条目。然后它打开拉取请求 “Kumi X.Y.Z”，不等其 CI，用管理员绕过将其合并为 “Kumi X.Y.Z (#PR)”，给合并提交打上标签 `vX.Y.Z` 并推送，再创建带发布说明的草稿发布（`--summary` 可以在说明开头加一句话）。
- **发布中的 Logic Pro 桥接：** CHANGELOG 和发布说明用它自己的话说明它的新版本：“Ships with Logic Pro bridge a.b.c, which Kumi starts beside Logic Pro; `kumi update` puts it in Kumi's own folder”；第一次带有它的版本则说“Brings Logic Pro bridge a.b.c for Logic Pro 12 on Apple Silicon Macs: `kumi --daw logic bridge` sets it up”。它说明随附版本的那一行与 Live 的并列，位于三个 `KUMI_CHANGES.md` 的“桥接版本”下（`Kumi X.Y.Z with Logic Pro bridge a.b.c.`，每种语言各一行，或者每种语言都没有）。第一次带有这个桥接的版本会发现那一行已经写明了它，于是保留它；之后的版本会像更新 Live 的那一行一样更新它。
- **标签触发的 Installer 运行**会为 macOS、Linux 和 Windows 的 Intel 与 ARM 构建原生发行包，测试安装和迁移，附加各平台的发行包和清单以及兼容用的 `kumi.tar.gz`、`kumi-release.json`、`SHA256SUMS` 和安装程序，并在说明由 `release.py` 撰写（说明以 `<!-- kumi:release-notes -->` 结尾）且两个安装程序都已附加时发布该版本。只有在此之后，安装程序、`kumi update` 和更新检查才能看到它。其他草稿，例如手动推送、说明“待补充”的标签，在有人发布之前一直是草稿。

**在本地准备原生发布**，需从一个干净的提交开始：

```sh
python3 scripts/build-hands.py              # 仅限 macOS；通用、临时签名的辅助程序，输出到 target/hands/
python3 scripts/build-logic-hands.py        # 仅用于搭载 Apple 芯片的 Mac 的发行包：Logic Pro 的辅助程序，输出到 target/logic-hands/
MACOSX_DEPLOYMENT_TARGET=13.0 python3 scripts/build-native-release.py --target aarch64-apple-darwin --out release/native/aarch64-apple-darwin
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

使用本机的 Rust 目标三元组；在 Apple Silicon 的 Mac 上，`--target x86_64-apple-darwin` 可以交叉构建 Intel 的发行包。构建器会运行锁定的 release 构建，并把桥接产物绑定到提交、Cargo 锁文件、构建方法和精确的文件哈希。CI 在拉取请求和 `main` 上使用的 `--profile ci-release` 省去 release 配置的全程序优化，以便更快地构建；发布的发行包始终使用默认的 `release`。Mac 发行包需要 `target/hands/` 中当前的辅助程序；其他发行包不包含它。搭载 Apple 芯片的 Mac 的发行包还在 `daws/logic/logic-kumi/` 中带有 Logic Pro 桥接（`logic-kumi`、它的辅助程序和一个 `version` 文件，即该 crate 的版本），这需要 `target/logic-hands/` 中只有当前源代码对应的那个辅助程序；其他发行包都不带它。服务器和分析工作进程必须一起发布。`--binaries-dir` 选项会打包已有的二进制文件，但不能证明它们是用 release 优化构建的。

聚合步骤会保留现有 Node 24 安装所使用的清单：

```sh
python3 scripts/build-migration-release.py release/native/*/kumi-release.json --node 24.21.0 --out release/installer
export KUMI_LEGACY_APP="$(python3 scripts/fetch-legacy-release.py)"
KUMI_NATIVE_RELEASES="$PWD/release/installer" python3 -m unittest discover -s scripts/tests -p test_migration_release.py
```

请使用发布工作流实际选定的 Node 24 版本。迁移测试会运行最后一个 JavaScript 版本 Kumi 1.7.5：`fetch-legacy-release.py` 下载它已发布的发行包，检查其 SHA-256，解包并打印所在文件夹。请用 Node 24 并设置这两个变量来运行，确保每个测试都会运行。

全新安装选择原生目标平台，不下载 Node。现有的 Node 24 安装会在 `kumi update` 时得到一个小型兼容引导程序（`scripts/migration/kumi.mjs`，每个发行包都把它附带为 `apps/kumi/bin/kumi.mjs`），它只下载并解包本平台的原生发行包。托管的 Node 会保留，供回滚和可选的 YouTube 挑战处理使用。在 Windows 上，原生版首次启动时会替换旧启动器；仍在运行旧启动器的 cmd 会从新启动器的填充行处继续并退出。更早的 Node 主版本可能需要使用相同 `KUMI_HOME` 重新运行安装程序。

**桥接**没有单独的发布：它随每个 Kumi 版本一起发布。[分发](DISTRIBUTION_POLICY.md)介绍了一个发布版本包含什么，以及如何检查。

## 文档

`docs/en` 中的每篇文档在 `docs/ja` 和 `docs/zh-CN` 中都有日文和中文版本，README 也有 `README.ja.md` 和 `README.zh-CN.md`。对其中一种语言的修改，要在同一个拉取请求中同步到全部三种语言。桥接的文档（它的 README，以及 `scripts/build-native-release.py` 的 `DOCUMENTS` 中列出的十四篇 `docs/en` 页面）会随桥接一起打包，所以它们的名称是固定的，链接也必须能够解析；打包测试会检查它们（见[测试](TESTING.md#文档)）。

## 前人的工作

桥接最初是在其他 Ableton MCP 服务器的基础上开始的：[bschoepke/ableton-live-mcp](https://github.com/bschoepke/ableton-live-mcp)、[uisato/ableton-mcp-extended](https://github.com/uisato/ableton-mcp-extended)、[Simon-Kansara/ableton-live-mcp-server](https://github.com/Simon-Kansara/ableton-live-mcp-server)、[jasper-zheng/ableton-sdk-mcp](https://github.com/jasper-zheng/ableton-sdk-mcp) 和 [ahujasid/ableton-mcp](https://github.com/ahujasid/ableton-mcp)。
