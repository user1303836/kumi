# 安装桥接

[English](../en/DELIVERY.md) · 简体中文 · [日本語](../ja/DELIVERY.md)

桥接由两部分组成：由 Live 加载的 `AbletonMcpBridge` Remote Script，以及由 Kumi（或其他 MCP 客户端）启动的本地 MCP 服务器。两者都在同一个包 `@ableton-mcp/mcp-server` 中，并由同一个工具安装：桥接的生命周期 CLI `ableton-mcp-server lifecycle`。它在做任何修改之前先制定计划，把安装的内容记录在回执中，并且能够精确地修复、回滚和移除这些内容。使用 Kumi 时，`kumi bridge` 会替你运行它。

## 使用 Kumi 安装

退出 Live，然后运行 `kumi bridge`。它会：

1. 在 Live 运行时拒绝执行，并请你确认 Live 已关闭（`--yes` 可事先确认）；
2. 从 Kumi 发行包中把桥接的包复制到它专用的文件夹（在源码副本中则改用 `scripts/build-native-release.py --bridge-only` 打包），并检查其哈希；
3. 运行生命周期的 `install`，如果已经装有桥接则运行 `upgrade`：先给出计划，再执行修改；
4. 把 Kumi 的 Live 扩展放进 Live 的 Extensions 文件夹，Live 12.4 及更高版本会运行它；
5. 最多等待十分钟，等 Live 通过新桥接连接上来，期间每隔几秒运行一次生命周期的 `activate`。

之后首次打开 Live 时，请在 **Settings → Link, Tempo & MIDI** 中将 **AbletonMcpBridge** 选为 Control Surface。在有未提交修改的源码副本中，`kumi bridge --allow-dirty` 仍会安装（仅供开发者使用）。

当桥接需要更新且 Live 已关闭时，`kumi update` 会运行 `kumi bridge`。原生应用首次启动时，还会迁移桥接版本相同的旧 JavaScript 桥接，并保留与回执绑定的配置、密钥和端口。`kumi uninstall` 会提出通过生命周期的 `uninstall` 把桥接和扩展从 Live 中移除，并且在 Live 仍从桥接的文件加载时保留这些文件。`kumi doctor` 检查整条链路。[Kumi 指南](KUMI_GUIDE.md#连接-live)从制作人的角度介绍了这些内容。

| 内容 | 位置 |
| --- | --- |
| 桥接的包 | `~/.kumi/bridge/<version>-<time>/package`（较早的 Node 世代保留原来的 `node_modules` 布局） |
| 它的状态：密钥、配置、回执、操作日志 | `~/.kumi/bridge/state`，或现有所有者回执所在的状态文件夹 |
| Remote Script | User Library 的 Remote Scripts 文件夹中的 `AbletonMcpBridge`（见 [Live 的文件夹](#live-的文件夹)） |
| Kumi 的 Live 扩展 | Live 的 Extensions 文件夹中的 `kumi.kumi` |

`KUMI_REMOTE_SCRIPTS_DIR` 和 `KUMI_LIVE_EXTENSIONS_DIR` 覆盖这两个 Live 文件夹，`KUMI_HOME` 改变 `~/.kumi` 的位置，`KUMI_BRIDGE_WAIT_SECONDS` 设置等待 Live 的时长（`0` 表示不等待）。Kumi 通过 `bridge-reference.json` 找到已安装的桥接，该文件由生命周期写在 Remote Script 旁边。

## 独立桥接

供 Kumi 以外的 MCP 客户端使用时，请使用适合你平台的原生归档包，不需要单独的 Node 运行时。在干净的源码副本中这样构建一个：

```sh
python3 scripts/build-native-release.py --bridge-only --out release/bridge
```

输出包含一个对应目标平台的 `.tar.gz`，以及记录其精确 SHA-256 的 `prepared.json`。用未提交的修改构建的包需要加上 `--allow-dirty-private-build`。

把归档包解压到一个长期存放的目录中。让 `ableton-mcp-server` 和 `ableton-mcp-analysis-worker` 与包中的资源和发布清单放在一起。以 macOS 为例：

```sh
ARTIFACT=/absolute/path/to/ableton-mcp-server-1.0.74-aarch64-apple-darwin.tar.gz
ARTIFACT_SHA="$(shasum -a 256 "$ARTIFACT" | awk '{print $1}')"
INSTALL_ROOT="$HOME/Library/Application Support/AbletonMcp/package"
STATE="$HOME/Library/Application Support/AbletonMcp/state"
REMOTE_SCRIPTS="$HOME/Music/Ableton/User Library/Remote Scripts"
mkdir -p "$INSTALL_ROOT" "$REMOTE_SCRIPTS"
tar -xzf "$ARTIFACT" -C "$INSTALL_ROOT"
PACKAGE_ROOT="$INSTALL_ROOT/package"
SERVER="$PACKAGE_ROOT/ableton-mcp-server"

"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA"
# 阅读计划，退出 Live，然后：
"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA" \
  --apply --confirm-live-stopped
```

在 Windows 上，用 `tar -xzf` 解压对应的归档包，然后用同样的选项和你的 Windows 绝对路径运行 `& "$PackageRoot\ableton-mcp-server.exe" lifecycle install`。`Get-FileHash -Algorithm SHA256 $Artifact` 会给出归档包的哈希。请使用你的 User Library 的 Remote Scripts 文件夹（见 [Live 的文件夹](#live-的文件夹)）。

打开 Live，将 **AbletonMcpBridge** 选为 Control Surface，然后用同样的 `--remote-scripts-dir`、`--state-dir` 和 `--package-root` 运行 `activate`。用 `--config <state>/bridge-config.json` 让 MCP 客户端指向已安装的服务器；见[用户指南](USER_GUIDE.md)。

升级时，把新的 tarball 解压到一个新目录中，用它的包根目录、产物和哈希运行 `upgrade`，并沿用现有的状态、配置和密钥路径。经过验证的旧 Node 安装可以迁移到桥接版本相同的原生包；其他升级必须提高版本。请保留上一个包以便回滚。删除包目录之前先运行 `uninstall`。生命周期仍然接受旧的 Node 发布产物和回执；这些需要 Node 22 或 24。

## 生命周期 CLI 参考

```text
ableton-mcp-server lifecycle <action> --remote-scripts-dir DIR [options]
```

| 操作 | 作用 | 需要 |
| --- | --- | --- |
| `install` | 创建仅限所有者访问的密钥和桥接配置，安装 Remote Script，写入回执 | `--artifact`、`--artifact-sha256`；Live 已停止 |
| `activate` | 在不改动 Live 或安装的情况下，检查 Live 是否加载了这个桥接并通过它应答；把结果记录在回执中 | Live 正在运行，已选择 Control Surface |
| `upgrade` | 用更新的包替换桥接，保留密钥，并保留上一个版本以供 `rollback` | 更新的产物，或经过验证的同版本 Node → 原生迁移；它的 SHA-256 和包根目录；Live 已停止 |
| `repair` | 把已安装的内容与回执比较；加上 `--apply` 时，把被改动的文件移到隔离区，并恢复包自带的文件 | — |
| `rollback` | 回到上次升级时保留的版本 | Live 已停止 |
| `uninstall` | 移除回执所拥有的文件；把被改动或未知的文件移到隔离区；保留密钥 | Live 已停止 |
| `status` | 只读报告：回执、文件完整性、漂移、权限、能否回滚 | — |

| 选项 | 含义 |
| --- | --- |
| `--remote-scripts-dir DIR` | Live 的 Remote Scripts 文件夹（必需） |
| `--state-dir DIR` | 密钥、配置、回执和操作日志所在的位置。默认为 `~/.config/ableton-mcp`，在 Windows 上为 `%APPDATA%\ableton-mcp` |
| `--package-root DIR` | 要使用的已安装包。默认：本 CLI 所属的包 |
| `--artifact FILE`、`--artifact-sha256 HEX` | tarball 及其哈希；生命周期会对照 tarball 自身的清单检查已安装的包 |
| `--config FILE`、`--secret FILE` | 配置和密钥的其他路径（默认：状态文件夹中的 `bridge-config.json` 和 `bridge.secret`） |
| `--host`、`--port`、`--realtime-port` | 新安装使用的回环地址和端口：默认为 `127.0.0.1`（或 `::1`）、9765 和 9766 |
| `--timeout-ms N` | 写入配置的桥接请求超时（默认 5000） |
| `--apply` | 执行修改。没有它时，每个操作都只制定计划 |
| `--confirm-live-stopped` | 表示你已退出 Live；带 `--apply` 的 `install`、`upgrade`、`rollback` 和 `uninstall` 需要它 |
| `--purge-secret` | 与 `uninstall` 一起使用：同时删除密钥，前提是该密钥由生命周期创建 |
| `--enable-bridge-diagnostics` | 与 `install` 一起使用：开启 Remote Script 的诊断日志 |
| `--allow-dirty-private-build` | 接受用未提交的修改构建的包（仅供开发者使用） |

每次运行都会在 stdout 上输出一个 JSON 结果（`ableton-mcp-lifecycle/v1`），其 `state` 为 `planned`、`completed`、`activation-required`、`blocked` 或 `failed`。被拒绝时则改为在 stderr 上输出 `ableton-mcp-lifecycle-error/v1`，其中的路径已被移除。被阻止、失败和被拒绝的运行以 2 退出。回执的状态在安装、升级、修复和回滚之后为 `installed-restart-required`，在 `activate` 通过桥接连上 Live 之后为 `activated`，移除之后为 `uninstalled`。

生命周期从不退出或启动 Live，从不选择 Control Surface，从不猜测 Live 的文件夹，也从不跟随给定路径中的符号链接或联接点（junction）。它在工作时持有锁，并为最近一次修改保留操作日志；中途失败时会恢复原有内容。运行被中断后，请先查看 `status` 和操作日志再重试，并按它们的提示使用 `repair` 或 `rollback`。

各操作的更多说明：

- **安装**（`install`）在做任何修改之前，会对照哈希检查 tarball 的字节、对照 tarball 的清单检查包，并检查端口是否空闲。它会在 Remote Script 的文件夹中放一个名为 `__pycache__` 的空文件，使 Live 无法写入或加载它的编译副本；该位置上的其他任何东西都算作漂移。
- **激活**（`activate`）只有在收到真实 Live 带有预期注册表哈希的经认证应答后，才会记录 `activated`。模拟器、过期或错误的注册表，或者没有应答，都会得到 `activation-required`，并说明下一步该做什么。已记录的激活只是历史，并不证明 Live 现在已连接。
- **升级**（`upgrade`）要求严格更新的版本，并拒绝已漂移的文件。它会保留上一个版本和配置以供 `rollback`。
- **修复**（`repair`）从不创建缺失的密钥，因为新的密钥就意味着对桥接的新授权。再次运行它不会做任何修改。
- **卸载**（`uninstall`）会保留密钥（除非使用 `--purge-secret`），并保留诊断日志。删除只是普通的 unlink，而不是安全擦除。

**诊断日志。** 安装时加上 `--enable-bridge-diagnostics`，Remote Script 会把简短、已脱敏的记录写入状态文件夹中的 `bridge-diagnostics.log`：仅限所有者访问，排队后在后台写入，最多 16 MiB。不加这个标志就没有日志。

## Live 的文件夹

| 文件夹 | macOS | Windows |
| --- | --- | --- |
| Remote Scripts（默认 User Library） | `~/Music/Ableton/User Library/Remote Scripts` | `Documents\Ableton\User Library\Remote Scripts`（或在 `OneDrive\Documents` 下） |
| Extensions（Live 12.4 或更高） | `~/Library/Application Support/Ableton/Extensions` | `%LOCALAPPDATA%\Ableton\Extensions`（尚未确认） |
| Control Surface 设置 | Live → Settings → Link, Tempo & MIDI | Options → Settings → Link, Tempo & MIDI |

如果你移动过 User Library，Live 的 **Settings → Library** 会显示它的位置；Kumi 会从 Live 的偏好设置中自行找到它。切勿安装到 Live 的应用程序文件夹中。

## 检查安装

```sh
ableton-mcp-server diagnostics --config /absolute/path/to/bridge-config.json
```

它会输出一份 JSON 报告。遇到不受支持的系统时以 1 退出；即使无法连接 Live 也以 0 退出，因此请阅读它的各个字段：

| 字段 | 含义 |
| --- | --- |
| `runtime`、`runtimeVersion`、`runtimeSupported`、`platformSupported` | 原生 Rust 运行时、桥接版本和平台支持情况 |
| `readiness.package` | 包及其 Remote Script 文件存在且完好（并不表示 Live 已加载它们；`status` 检查的是已安装的副本） |
| `readiness.configured` | 配置有效，指定了桥接，并且有可读取的密钥 |
| `readiness.authenticatedBridge` | Remote Script 通过经认证的连接作出了应答，且探查成功（`registryHash` 显示其注册表） |
| `readiness.realLiveOperational` | 该应答来自真实的 Live（`real-live` 来源），而不是模拟器 |
| `ready` | 以上全部满足 |

密钥永远不会被输出。重新安装并不能修复连接问题：请检查 Live 是否已重启、是否已选择 Control Surface，以及配置、密钥和端口是否一致。

## 把配置迁移到版本 2

默认情况下，`ableton-mcp-server migrate` 会原样保留旧的（旧式或版本 1 的）客户端配置。如果提供了所有桥接字段和一个已存在的、仅限所有者访问的密钥，它会写出一份版本 2 的桥接配置：

```sh
ableton-mcp-server migrate --input /absolute/old.json --output /absolute/bridge-v2.json \
  --bridge-host 127.0.0.1 --bridge-port 9765 --realtime-port 9766 \
  --secret-file /absolute/bridge.secret
```

它从不创建密钥，只接受回环主机，并且在没有 `--force` 时拒绝替换已存在的文件。
