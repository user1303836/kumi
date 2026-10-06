# 用户指南

[English](../en/USER_GUIDE.md) · 简体中文 · [日本語](../ja/USER_GUIDE.md)

桥接是 Kumi 与 Ableton Live 之间的连接，任何 MCP 客户端也可以单独使用它。它由两部分组成：

- 一个本地 MCP 服务器 `@ableton-mcp/mcp-server`；
- 一个在 Live 12 内部运行的 Remote Script `AbletonMcpBridge`。

服务器通过经过认证的回环连接与 Remote Script 通信。这样，客户端就可以读取当前打开的工程并修改它：每项修改都先预览、再应用，需要时可以撤销。客户端还可以播放、录制和分析音频。

本指南介绍安装设置、配置、部署策略、修改的工作方式以及所有工具。如果你使用 Kumi，`kumi bridge` 会替你完成全部设置；见 [Kumi 指南](KUMI_GUIDE.md)。

## 安装

原生服务器以可执行文件的形式在 macOS、Windows 或 Linux 上运行。连接 Live 需要 macOS 或 Windows。请使用与你的操作系统和架构对应的归档包；分析工作进程必须留在服务器旁边。不需要单独的 Node 运行时。

- **使用 Kumi：** 关闭 Live，然后运行 `kumi bridge`。
- **独立使用：** 按照[交付](DELIVERY.md)中的说明，通过 `ableton-mcp-server lifecycle` 安装原生 tarball。
- **从源码构建：** 在仓库根目录运行 `cargo build --release --locked -p ableton-mcp-server --bins`。在得到配置之前，`target/release/ableton-mcp-server` 启动后只提供离线工具。

## 连接到 Live

生命周期安装程序会创建仅所有者可访问的密钥和配置，安装 Remote Script，并保留一份回执用于升级和回滚。在 Windows 上也请使用它，这样它会设置所需的文件权限。请按照[交付](DELIVERY.md)操作。

然后打开 Live，在 **Settings → Link, Tempo & MIDI** 中选择 **AbletonMcpBridge**。用下面的命令检查连接：

```sh
/absolute/path/ableton-mcp-server diagnostics --config /absolute/path/bridge-config.json
```

查找 `"provenance": "real-live"` 和 `"readiness": { … "realLiveOperational": true }`。即使没有连接，诊断也可能以成功状态退出；请阅读报告中的 readiness 字段。

### 配置文件

`ableton-mcp-server setup` 会写出版本 2 的文件。服务器、Remote Script 和生命周期工具都会读取它：

```json
{
  "version": 2,
  "server": {
    "command": "/absolute/path/ableton-mcp-server",
    "args": ["--config", "/absolute/path/bridge-config.json"]
  },
  "bridge": {
    "host": "127.0.0.1",
    "port": 9765,
    "secretFile": "/absolute/path/bridge.secret",
    "timeoutMs": 5000,
    "realtimePort": 9766
  }
}
```

| 字段 | 规则 |
| --- | --- |
| `server.command` | 原生服务器可执行文件的绝对路径 |
| `server.args` | `--config` 以及本文件自身的绝对路径 |
| `bridge.host` | `127.0.0.1` 或 `::1` |
| `bridge.port` | 1–65535；Remote Script 在此端口监听 |
| `bridge.secretFile` | 绝对路径；仅所有者可访问，至少 32 个字符 |
| `bridge.timeoutMs` | 每个发往 Live 的请求 100–60,000 ms（默认 5,000） |
| `bridge.realtimePort` | 可选；必须与 `port` 不同；见[实时控制](REALTIME_CONTROL.md) |
| `bridge.diagnostics` | 可选；只由 `ableton-mcp-server lifecycle install --enable-bridge-diagnostics` 写入（见[运维](OPERATIONS.md)） |

迁移期间，仍可读取写明 Node 和 `cli.js` 的旧版本 2 配置。未知字段会被拒绝。该文件必须只有你能读取。不带桥接选项运行 `ableton-mcp-server setup` 会写出版本 1 的文件。这种文件只说明如何启动服务器；传给 `--config` 时会被拒绝。`ableton-mcp-server migrate` 可以转换旧文件（见[交付](DELIVERY.md)）。

## 把桥接添加到 MCP 客户端

使用配置中的 `server.command` 和 `server.args`。以常见的 `mcpServers` 格式为例：

```json
{
  "mcpServers": {
    "ableton": {
      "command": "/absolute/path/ableton-mcp-server",
      "args": ["--config", "/absolute/path/bridge-config.json"],
      "env": { "ABLETON_MCP_TOOL_POLICY": "edit-no-audio" }
    }
  }
}
```

服务器在 stdin 和 stdout 上以 JSON 行的形式收发 MCP 消息。它自己的日志行带 `mcp-host:` 前缀，写到 stderr。

## Kumi 的 Live 扩展

在 Live 12.4 及更高版本上，当 Kumi 的 Live 扩展正在运行时，桥接也会连接到它。这会增加：

- 离线渲染；
- 直接写入编曲视图的 MIDI 片段；
- 清空编曲视图中的一段区域；
- 复制设备；
- 把文件导入工程；
- “Ask Kumi about this” 右键事件。

扩展有两种运行方式：

- **安装在 Live 的 Extensions 文件夹中。** `kumi bridge` 会把它放在那里，Live 打开时会启动它。
- **由桥接启动**，通过 Live 自己的 Extension Host 运行，前提是 Live 的 Developer Mode 已开启（Settings → Extensions）。

Live 连接期间，桥接每 10 秒寻找一次扩展。扩展一旦应答，它的工具就会出现在 `tools/list` 中；见[工具参考](#live-扩展工具)。其他功能都不依赖扩展。

| 变量 | 作用 |
| --- | --- |
| `ABLETON_MCP_EXTENSION=off` | 不连接扩展 |
| `ABLETON_MCP_EXTENSION=external` | 连接正在运行的扩展，但从不自行启动扩展 |
| `ABLETON_MCP_EXTENSION_DIR` | 由桥接启动的扩展存放其端点、密钥和渲染文件的位置（默认：配置文件旁边的 `live-extension`） |
| `ABLETON_MCP_LIVE_EXTENSIONS_DIR` | Live 的 Extensions 文件夹，如果不在默认位置（`~/Library/Application Support/Ableton/Extensions`、`%LOCALAPPDATA%\Ableton\Extensions`） |

## 命令

| 命令 | 选项 |
| --- | --- |
| `ableton-mcp-server` | 无，或恰好一个 `--config PATH` |
| `ableton-mcp-server setup` | `--output PATH`；版本 2 还需要 `--bridge-port N`、`--secret-file PATH`，以及可选的 `--bridge-host`、`--bridge-timeout MS`、`--realtime-port N`。`--force` 会覆盖已有文件。 |
| `ableton-mcp-server install-remote-script` | `--destination DIR`、`--config PATH`、`--dry-run`、`--force` |
| `ableton-mcp-server diagnostics` | 无，或恰好一个 `--config PATH`；输出一份 JSON 报告 |
| `ableton-mcp-server lifecycle`、`ableton-mcp-server migrate` | 见[交付](DELIVERY.md) |

前四个命令遇到错误选项时以 2 退出，执行失败时以 1 退出。

## 协议

服务器支持两个 MCP 协议版本。每个服务器进程只用其中一个（在 `2025-11-25` 的 `initialize` 之前可以先发一个 `server/discover`）。

- **`2025-11-25`：** 先发送 `initialize`，再发送 `notifications/initialized`。服务器会发送 `notifications/tools/list_changed` 和 Live 事件（见[事件](#事件)）。
- **`2026-07-28`：** 没有握手。每个请求都在 `params._meta` 中携带 `io.modelcontextprotocol/protocolVersion` 和 `io.modelcontextprotocol/clientCapabilities`；`server/discover` 是可选的：

  ```json
  {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}
  ```

  结果带有 `resultType: "complete"`，工具结果还会把 JSON 同时放在 `structuredContent` 中。列表和资源都标记为 `ttlMs: 0`：请重新读取，不要缓存。这个版本没有推送通知。请用 `live_observe_poll` 代替 `live_subscribe`。

`tools/list` 只显示当前连接的 Live 所支持、且部署策略此刻允许的工具。这个列表会随着 Live 连接、断开或重连，以及 Live 扩展的出现与消失而变化。在 `2025-11-25` 下，服务器会用 `notifications/tools/list_changed` 通告每次变化。`capabilities` 和 `ableton://capabilities` 资源还会列出哪些工具被隐藏，以及原因。

## 部署策略

部署策略决定客户端能看到和调用哪些工具。在服务器的环境变量中设置：

| 变量 | 值 |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY` | 一个配置档：`read-only`、`edit-no-audio`、`performance` 或 `full`（默认） |
| `ABLETON_MCP_TOOL_ALLOW` | 以逗号分隔的工具名或 `prefix*` 模式；在配置档范围内只允许这些 |
| `ABLETON_MCP_TOOL_DENY` | 以逗号分隔的、永远不允许的名称或模式；拒绝始终优先 |

| 配置档 | 允许 |
| --- | --- |
| `read-only` | 本地工具和读取；不做任何修改 |
| `edit-no-audio` | 读取加编辑：结构、MIDI、设备、调音台、自动化、路由。不包括任何会播放、录音、捕获、写文件或运行 Python 的操作。 |
| `performance` | 读取加播放、视图与选择、调音台、速度、`live_undo` 和 `live_recovery_finalize` |
| `full` | 所有类别，包括 `python` |

每个工具都属于一个类别，列在[工具参考](#工具参考)中：`local`、`read`、`edit`、`performance`、`audio`、`filesystem`、`recording`、`realtime`、`capture` 或 `python`。类别与工具实际行为不一致的地方：

- `live_render_offline` 属于 `read`，尽管它会写出渲染文件。
- `.als` 工具属于 `filesystem`，尽管它们只做读取。
- `live_change` 属于 `edit`，所以 `performance` 不包含它。

每次调用时都会重新检查策略。如果某项修改所用的工具已不再被策略允许，对它的 `live_undo` 会被拒绝。格式错误的值会让服务器在启动时停止。`ableton-mcp-diagnostics` 会报告当前生效的策略。

对于不完全信任的客户端，请从 `read-only` 或 `edit-no-audio` 开始。在 `full` 下，请拒绝 `live_run_python`，它可以在 Live 内运行任意 Python。

## 修改的工作方式

### 预览、应用、撤销

一项修改分三步：

1. **读取**要修改的对象（`live_discover`、`live_snapshot`），得到它的引用（ref）。
2. 用 `*_preview` 工具**预览**。预览不会改变任何东西。它返回将会发生的变化、一个 `transactionId`、一个 `confirmation` 和 `expiresAt`。
3. 用对应的 `*_apply` 工具**应用**：传入 `transactionId`、`confirmation`，以及你自己选定的 `idempotencyKey`（8–128 个字符）。桥接会在 Live 的线程上确认自预览以来没有任何变化，然后应用修改并读回结果。

用同一个键再次发送同一个应用请求，会再次得到应答（`"idempotent": true`），而不会应用两次。保留 `transactionId`，以便用 `live_undo`（`confirmation: "undo"`）撤销这项修改。

`live_change` 在一次调用中完成预览和应用：`{"tool": "live_mixer_preview", "args": {…}}`。它返回应用的结果（预览的结果在 `preview` 下），所以 `live_undo` 照常可用。对于应当先让人看到再发生的修改，它会拒绝：试听、片段触发、触发按钮、捕获、录音、启用实时通道以及 Live 的对话框。

### 确认与过期

- 大多数预览给出的确认是 `"apply"`。
- 场景试听和片段触发会给出一个不可预测的令牌，外加一个单独用于停止的令牌。捕获会给出一个不可预测的令牌。
- 少数工具使用自己的确认词：`"undo"`、`"backup"`、`"disarm"`、`"undo-in-live"`、`"redo-in-live"`、`"emergency-stop"`、`"emergency-stop-and-clean"` 和 `"finalize-recovery-record"`。

预览在 10 分钟后过期。批量、MIDI 片段和设备状态的预览在 30 秒后过期，捕获预览在 60 秒后过期。过期后请重新预览。

会播放或录音的工具在其 schema 中有一个 `outputSafety` 对象（`{"safe": true, "provenance": "…"}`）。只有场景试听必须提供它。对其他工具，客户端没有提供时桥接会使用自己的值；但会强制校验 schema 的客户端，仍须在 schema 要求的地方发送它。

### 桥接会做的调整

- 超出范围的参数值会取最近的端点。介于步进参数两档之间的值会取最近的一档。只有 Live 中呈灰色不可用的参数会被拒绝。
- 轨道和场景可以重名，但批量操作新建的轨道除外。
- 没有 `seed` 的随机 MIDI 变换会从请求中推导出一个，因此预览和应用的结果一致。

### 修改被拒绝时

被拒绝的调用会返回 `isError: true` 以及 `{"reason": "...", "remediation": "..."}`。原因（reason）是桥接或 Live 自己给出的。

- “Nothing changed in Live”：按原因所说的去修正，然后重新预览。
- “Live state changed since the preview”：重新读取，然后重新预览。
- 超时、丢失应答或读回失败会让修改处于不确定状态。只能用同一个键重试同一个应用请求；见[恢复](RECOVERY.md)。

### 撤销

无论对象此后发生了什么变化，`live_undo` 都能撤回修改：被改名的轨道、又被推动过的推子、加了片段的设备。如果该引用现在指向的是另一个对象，它会拒绝。

有些修改没有 `live_undo`：

- 删除、清空编曲视图中的一段区域、`live_run_python`，以及其他预览中说明会直接保留的修改（裁剪、存储的机架变体、清空的打击垫）。Live 自己的撤销（`live_song_undo`）可以撤回这些修改。
- 播放：触发、触发场景、走带操作、触发按钮。请改为停止它们。

`live_undo_step_begin` 和 `live_undo_step_end` 会把多项修改合并成 Live 中的一次 Cmd-Z。这个步骤会在 `timeoutMs`（默认两分钟）后、连接断开时或另一个步骤打开时自行关闭。通过 Live 扩展做出的修改不会被合并。

服务器会保存已应用修改的撤销记录：总计最多 1 GiB，批量、MIDI 片段和设备状态修改各最多 512 条。超出后，最早应用的修改会放弃它的撤销。`live_transaction_release` 可以放弃你不会用到的撤销。撤销记录保存在服务器内存中，重启后会丢失。

[Live 安全](LIVE_SAFETY.md)说明了桥接提供哪些保证，以及哪些工具不经过预览和应用就能工作。

## 工具参考

服务器能提供的全部工具。`tools/list` 只显示当前连接的 Live 所支持、且策略允许的那些。`name_preview/apply` 表示 `name_preview` 和 `name_apply` 这一对；`/stop` 表示另加 `name_stop`。“类别”是指[部署策略](#部署策略)中的类别。

### 状态与离线工具

这些工具无需 Live 即可工作，`live_status` 除外，它报告 Live 是否在线。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `server_status` | local | 服务器的版本，以及是否连接了 Live 适配器。 |
| `capabilities` | local | 协商后的能力，以及哪些工具可执行、可见或被策略拒绝。 |
| `live_status` | read | Live 的连接情况：协议、适配器、来源（`real-live`）、epoch、注册表哈希、能力和操作。如果桥接已断开，会先重连。始终列出。 |
| `plan_user_journey` | local | 为五个引导流程之一生成计划；不做任何修改。见[操作示例](USER_JOURNEYS.md)。 |
| `audio_analyze` | local | 分析你发送的 float32 PCM 的响度（BS.1770-5 / EBU R128）、真峰值、频谱、动态和削波。 |
| `audio_compare_reference` | local | 将你的 PCM 与参考音频比较：对齐、电平匹配和差异。 |
| `als_read/lint/diff` | filesystem | 在你指定的 `allowedRoot` 内读取、检查或比较已保存的 `.als` 文件，无需 Live。 |
| `live_project_snapshot_diff` | read | 比较两个导出的工程快照，无需 Live。 |
| `live_library_search` | read | 在你允许的文件夹中搜索 Live 自己的资源库数据库（文件、标签、插件）；只读。 |

### 读取工程

这些读取返回的引用（`<epoch>:track:4` 等）就是修改工具要用的参数。引用在 Live 的 epoch 改变之前一直有效。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_snapshot` | read | 整个工程的一份有大小上限的快照。 |
| `live_discover` | read | 分页列出某一类对象：`set`、`track`、`return-track`、`main-track`、`scene`、`clip-slot`、`session-clip`、`arrangement-clip`、`note`、`locator`、`device`、`parameter`、`selection`、`routing-choice`、`session-playback`。片段槽、片段、音符、参数和路由选项需要 `parent` 引用。最多 8 个过滤条件，可指定 `fields`、`limit`、`cursor`。 |
| `live_song_state` | read | 歌曲级状态：拍号、摇摆、录音和叠录模式、预备录音和独奏模式、Link。 |
| `live_performance_read` | read | CPU 占用、轨道电平表和设备延迟，采样一次。 |
| `live_note_read` | read | 按 id 读取 MIDI 片段的音符，或读取选中的音符。 |
| `live_key_estimate` | read | 为 MIDI 片段或一组音符给出排序后的候选调性。 |
| `live_project_info` | read | 已保存工程的文件、它引用的媒体以及缺失的内容。 |
| `live_project_snapshot_export` | read | 经过隐私过滤的工程快照（`strict`、`collaboration` 或 `local`）的一页，可保存下来供以后比较。 |
| `live_automation_read` | read | Session 片段中某个参数的包络，以及它在某一拍上的值。 |
| `live_arrangement_automation_read` | read | 编曲视图片段中某个参数的包络点。 |
| `live_take_lane_read` | read | 轨道的 take lane 及其中的片段。 |
| `live_comp_read` | read | 哪些 take lane 段落组成了一个 comp 片段。 |
| `live_warp_marker_read` | read | 音频片段的 warp 标记。 |
| `live_device_read` | read | 插件的全部参数名，或 Max for Live 设备的参数组（bank）。 |
| `live_clip_time_convert` | read | 在音频片段中于拍、采样帧和秒之间换算。 |
| `live_data_read` | read | 存储在工程或轨道上某个键下的文本。 |
| `live_browser_roots` | read | Live Browser 的根目录。 |
| `live_browser_search` | read | 按类别和关键词对 Live 的 Browser 进行排序搜索。 |
| `live_browser_inspect` | read | 按 id 查看单个 Browser 条目：它是什么，以及能否加载。 |

### 跟踪变化

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_subscribe` | read | 在 Live 变化时发送 `notifications/live_event`（见[事件](#事件)）。仅限旧版协议。 |
| `live_unsubscribe` | read | 停止这些通知。 |
| `live_observe_subscribe/poll/unsubscribe` | read | 一个由你轮询变化主题的观察器（走带、选择、轨道、片段、设备、参数、律动、调律、场景、电平表、机架）。两个协议版本都可用。 |

### 轨道、场景与结构

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_session_structure_preview/apply` | edit | 在你指定的位置创建 MIDI 轨道、音频轨道和场景。名称可以重复。 |
| `live_track_structure_preview/apply` | edit | 创建或删除返回轨道；复制轨道或场景。 |
| `live_scene_capture_preview/apply` | edit | 把正在播放的内容捕获到一个新场景中。 |
| `live_object_rename_preview/apply` | edit | 重命名轨道、场景、片段、设备、定位标记或 take lane。 |
| `live_track_properties_preview/apply` | edit | 轨道的颜色（调色板索引 0–69）。 |
| `live_scene_preview/apply` | edit | 场景的颜色、速度和拍号。 |

### 删除

删除会直接保留下来：`live_undo` 无法恢复，但 Live 自己的撤销（`live_song_undo`）可以。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_track_delete_preview/apply` | edit | 删除音频、MIDI 或编组轨道及其片段和设备；删除编组会连同其中的轨道一起删除。 |
| `live_scene_delete_preview/apply` | edit | 删除场景及其片段。工程至少保留一个场景。 |
| `live_clip_delete_preview/apply` | edit | 删除 Session 或编曲视图中的片段。 |
| `live_locator_delete_preview/apply` | edit | 删除定位标记。 |
| `live_device_delete_preview/apply` | edit | 删除设备。 |

### Session 片段、音符与片段自动化

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_midi_clip_preview/apply` | edit | 在空的 Session 槽位中创建带音符的 MIDI 片段。 |
| `live_note_update_preview/apply` | edit | 按 id 修改音符：音高、起点、长度、力度、静音、概率、力度偏差、释放力度。 |
| `live_note_delete_preview/apply` | edit | 按 id 删除音符。 |
| `live_note_edit_preview/apply` | edit | 量化或复制音符、选择音符，或删除某个音高和时间范围内的音符。 |
| `live_midi_transform_preview/apply` | edit | 变换和生成器：移调、音阶、量化、摇摆、人性化、琶音、欧几里得节奏、和弦进行、鼓型、贝斯线、动机倒影等。随机类操作接受 `seed`，或从请求中推导一个。生成器默认写入空槽位中的副本。 |
| `live_capture_midi_preview/apply` | edit | Live 的 Capture MIDI。 |
| `live_clip_properties_preview/apply` | edit | 片段的静音、颜色、MIDI 循环、触发模式和量化、legato、RAM 模式、力度量、律动。 |
| `live_clip_action_preview/apply` | edit | 裁剪、复制循环或某个区域、搓擦（scrub）、移动播放位置。 |
| `live_clip_duplicate_preview/apply` | edit | 把 Session 片段复制到另一个槽位或编曲视图中。 |
| `live_clip_move_preview/apply` | edit | 移动编曲视图中的片段，或把 Session 片段移到另一个槽位。 |
| `live_automation_preview/apply` | edit | Session 片段包络：创建或删除包络，插入或删除点，绘制阶梯，清除所有包络。 |

### 编曲视图

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_arrangement_section_preview/apply` | edit | 在一个段落两侧添加两个命名的定位标记。 |
| `live_arrangement_clip_preview/apply` | edit | 在编曲视图中创建一个空的 MIDI 片段，或从 `filePath`（按原样传给 Live）创建音频片段。 |
| `live_locator_jump_preview/apply` | performance | 把播放头移到下一个、上一个或指定的定位标记。 |

### 音频片段与文件

音频导入、Simpler 加载和打击垫加载都需要一个文件路径，以及文件必须位于其中的 `allowedRoot` 文件夹。桥接会检查文件，并把保存在受管文件夹中的副本交给 Live（见 [Live 安全](LIVE_SAFETY.md)）。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_audio_clip_preview/apply` | audio | 音频片段的增益、音高、循环、warp 和淡入淡出，以片段提供的选项为限。 |
| `live_warp_marker_preview/apply` | audio | 按拍时间添加、移动或删除 warp 标记。 |
| `live_audio_import_preview/apply` | filesystem | 把音频文件放进空的 Session 槽位或 take lane。MIDI 文件会被拒绝。 |
| `live_simpler_preview/apply` | filesystem | 替换 Simpler 的采样。 |
| `live_project_backup_preview/apply` | filesystem | 在已保存工程旁边生成一份经过校验的副本。预览需要 `confirmation: "backup"` 以及包含该工程的 `allowedRoot`。 |

### 设备、机架与 Browser

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_browser_load_preview/apply` | edit | 把 Browser 条目加载到轨道现有设备之后，或加载到机架的某条链中（`chainRef`）。同一轨道上的第二个乐器会被拒绝。 |
| `live_device_preview/apply` | edit | 按名称插入原生设备（Simpler 可以同时加载采样），开启或关闭设备，或移动设备。 |
| `live_device_parameter_preview/apply` | edit | 设置一个参数，或用 `values` 一次设置同一设备的最多 10,000 个参数。超出范围的值会停在端点，介于两档之间的值会取最近的一档。 |
| `live_device_state_save` | filesystem | 把设备或机架的参数值保存为 JSON 文件，放在你指定的文件夹中。 |
| `live_device_state_recall_preview/apply` | read, edit | 把保存的状态调回到设备上，或在两个状态之间渐变（morph）。 |
| `live_device_advanced_preview/apply` | edit | 参数组（bank）、重新启用自动化、A/B 保存、插入到链中、移到另一条轨道或链。 |
| `live_device_specialized_preview/apply` | edit | Drift（含其调制矩阵）、Drum Cell、EQ Eight、Hybrid Reverb、Meld、插件预设、Simpler 采样设置、Wavetable。 |
| `live_device_edit_preview/apply` | edit | 不属于参数的设置（Roar、Shifter、Spectral Resonator、Hybrid Reverb、CC Control、Simpler）、Simpler 切片和 warp、Wavetable 调制量。 |
| `live_device_io_preview/apply` | edit | 设备自身的输入或输出路由，或压缩器的侧链源。 |
| `live_chain_preview/apply` | edit | 机架链的颜色、静音和独奏。 |
| `live_chain_mixer_preview/apply` | edit | 机架链的音量、声像、发送和激活开关。 |
| `live_rack_preview/apply` | edit | 宏数量和变体；添加、移除或随机化宏；插入链；复制打击垫。 |
| `live_rack_view_preview/apply` | edit | 机架显示哪条链或哪个打击垫。 |
| `live_drum_pad_preview/apply` | edit | 打击垫的音符和独奏、清空打击垫，或把采样以 Simpler 或 Drum Sampler 的形式加载到打击垫上（一个或整个机架）。 |
| `live_looper_preview/apply` | edit | Looper 的操作和设置。 |

### 混音、路由与批量操作

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_mixer_preview/apply` | edit | 音量、声像、静音、独奏、预听（cue）和发送。 |
| `live_mixer_extended_preview/apply` | edit | 轨道激活开关、交叉推子及其分配、声像模式、分离立体声。 |
| `live_routing_preview/apply` | edit | 输入和输出路由、预备录音和监听。会形成反馈的路由会被拒绝。 |
| `live_batch_preview/apply` | edit | 最多 32 个调音台、参数、片段、重命名、新建轨道和预备录音操作，合为一项修改，共用一次撤销。新轨道的名称不能已被占用。 |

### 速度、歌曲设置、调律与律动

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_tempo_preview/apply` | edit | 速度，20–999 BPM。 |
| `live_song_settings_preview/apply` | edit | 拍号、摇摆、触发和录音量化、触发时选中（select on launch）。 |
| `live_tuning_preview/apply` | edit | 调律系统和音阶。 |
| `live_groove_preview/apply` | edit | 全局律动量和律动池中的律动。 |

### 播放

这些操作会发出声音。大多数无法撤销；要回到原状，就停止播放。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_transport_preview/apply` | performance | 歌曲位置、循环、穿插录音（punch）和节拍器（可撤销）。 |
| `live_transport_action_preview/apply` | performance | 开始、继续、停止、播放选区、停止所有片段、回到编曲视图、搓擦、敲击速度、微调（nudge）、跳转、触发 Session 录音。 |
| `live_clip_launch_preview/apply/stop` | performance | 触发一个片段（无论工程是否正在播放），并可再次停止这个片段。 |
| `live_scene_fire_preview/apply` | performance | 触发一个场景。 |
| `live_fire_button_preview/apply` | performance | 像控制器那样按下或松开片段、槽位或场景的触发按钮。 |
| `live_session_audition_preview/apply/stop` | performance | 受保护的场景试听：需要工程名称、输出安全证据，并且工程处于停止状态，没有任何轨道预备录音或监听输入。 |
| `live_session_emergency_stop` | performance | 停止你刚刚观察到的 Session 片段、走带和录音；不需要事务，重启后也能使用。 |
| `live_browser_preview` | performance | 播放 Browser 条目的试听，就像在 Live 中点击它一样。 |
| `live_browser_preview_stop` | performance | 停止该试听。 |

### 录音与捕获

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_recording_preview/apply` | recording | 开始或停止 Session 或编曲视图录音。目标轨道必须已预备录音；Live 也会录到其他已预备录音的轨道上。 |
| `live_audio_capture_preview/apply` | capture | 通过 Resampling 录制片段的 1–9 秒，分析后删除这段录音。仅限真实 Live；见[音频智能](AUDIO_INTELLIGENCE.md)。 |
| `live_audio_capture_status` | read | 捕获在其生命周期中所处的阶段。 |
| `live_audio_capture_emergency_stop` | capture | 在失败或重启后停止并清理捕获。 |
| `audio_diagnose_live_context` | read | 把你发送的 PCM 的测量结果与某条轨道当前的设备和调音台关联起来。 |

### 视图与 Live 界面

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_view_preview/apply` | performance | 显示 Session 或编曲视图；编曲视图的缩放、滚动和跟随。 |
| `live_track_view_preview/apply` | performance | 轨道折叠、设备插入模式、显示机架链、选中乐器。 |
| `live_selection_preview/apply` | performance | 选中轨道、场景、槽位、片段、设备、参数或链；绘制模式。 |
| `live_clip_view_preview/apply` | performance | 片段的网格、包络和循环显示。 |
| `live_device_view_preview/apply` | performance | 折叠或展开设备。 |
| `live_application_dialog_preview/apply` | edit | 读取 Live 当前打开的对话框，并按下其中一个按钮。 |
| `live_message` | performance | 在 Live 的状态栏中显示消息，或用 `modal: true` 以对话框显示。 |

### 撤销与记录管理

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_undo` | edit | 撤销一项已应用的修改（`confirmation: "undo"`）。 |
| `live_change` | edit | 在一次调用中运行预览及其应用：`tool`（一个 `*_preview`）、`args`、可选的 `idempotencyKey`。拒绝试听、片段触发、触发按钮、捕获、录音、启用实时通道和对话框。 |
| `live_undo_step_begin/end` | edit | 把两者之间的修改合并为 Live 自身撤销中的一步。 |
| `live_song_undo/redo` | edit | Live 自己的撤销和重做，执行一次（`undo-in-live`、`redo-in-live`）。用于删除以及在 Live 中做出的编辑。 |
| `live_transaction_release` | edit | 放弃最多 64 项你不会撤销的已应用修改的撤销记录。 |
| `live_recovery_finalize` | edit | 在你手动检查过 Live 后，关闭一项不确定修改的记录。见[恢复](RECOVERY.md)。 |

### 实时控制

用于快速改变参数的短时 UDP 通道。见[实时控制](REALTIME_CONTROL.md)。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_realtime_arm_preview/apply` | realtime | 为指定参数打开通道并返回其令牌。 |
| `live_realtime_disarm` | realtime | 关闭通道。 |
| `live_realtime_stats` | realtime | 已接收、已应用和已丢弃的数据包数。 |

### Python 与存储的文本

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_run_python` | python | 在 Live 主线程上运行 Python（`code`、`mode` 为 `exec` 或 `eval`、可选的 `ref`、`timeoutMs` 最多 30,000）。没有预览，也没有 `live_undo`；在 Live 的撤销中占一步。见 [Live 安全](LIVE_SAFETY.md)。 |
| `live_data_preview/apply` | edit | 在工程或轨道上以 `kumi.` 开头的键下存储文本。 |

### Live 扩展工具

桥接连接到 Kumi 的 Live 扩展时才会列出（见 [Kumi 的 Live 扩展](#kumi-的-live-扩展)）。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_render_offline` | read | 把音频轨道两拍之间的片段渲染为文件，不经过轨道上的设备，也无需播放。 |
| `live_project_import` | filesystem | 把音频文件复制到工程的项目文件夹中。 |
| `live_arrangement_midi_clip_preview/apply` | edit | 把一个或多个带音符的 MIDI 片段写入编曲视图。 |
| `live_clip_clear_range_preview/apply` | edit | 清空某条轨道在编曲视图中的一段区域，并在区域边缘切开片段。和删除一样会直接保留。 |
| `live_device_duplicate_preview/apply` | edit | 复制一个设备及其设置，放在它的紧后面。 |

### Willington 工具

只有在 Willington 的绑定开启（`/willington`）且 Live 构建版本受其覆盖时才会列出；见 [Willington](WILLINGTON_INTEGRATION.md)。

| 工具 | 类别 | 作用 |
| --- | --- | --- |
| `live_willington_device_preview/apply` | edit | 机架宏和变体的名称、宏映射以及链区域（chain zone）。 |
| `live_follow_actions_preview/apply` | edit | Session 片段的 Follow Actions，需在停止播放时设置。 |

## 事件

在 `2025-11-25` 下，`live_subscribe`（可附带 `types` 列表）会让服务器在 Live 变化时发送 `notifications/live_event`：

| 类型 | 何时发送 |
| --- | --- |
| `transport` | 播放或录音开始或停止 |
| `object` | 轨道或场景列表发生变化 |
| `selection` | 选择发生变化 |
| `name` | 轨道、场景或片段被重命名或改色 |
| `mixer` | 轨道的静音、独奏、预备录音、音量、声像或发送发生变化 |
| `parameter` | 选中设备的某个参数发生变化 |
| `structure` | 轨道、场景、定位标记，或轨道的设备或片段发生变化 |
| `reset` | 请重新读取 Live：你手上的数据可能已过时 |

每个事件都有 `epoch`、`sequence`、`type`、`channel`（`remote-script` 或 `extension`，各自独立编号）和 `payload`。`pointed` 事件来自扩展的 “Ask Kumi about this”，无需订阅。如果积压的事件超过 65,536 个，服务器会丢弃其余事件，并发送带 `resnapshot: true` 的 `notifications/live_event_overflow`。出现这种情况、收到 `reset` 或 `sequence` 出现断档后，请重新读取工程。

在 `2026-07-28` 下，请使用 `live_observe_subscribe` 和 `live_observe_poll`。

## 资源与提示词

| 资源 | 内容 |
| --- | --- |
| `ableton://capabilities` | 协商后的能力，以及哪些工具可用、可见或被策略拒绝，附带其类别（JSON） |
| `ableton://safety` | 简短的安全摘要（Markdown） |
| `ableton://journeys` | 五个引导流程，以及当前 Live 对每个流程的支持情况（JSON） |
| `ableton://live-workflow` | 安全修改速度的分步说明（Markdown） |
| `ableton://max-extension` | 运维人员自建的 Max 补丁可用于实时控制的数据包格式；不附带任何 Max 设备（JSON） |

| 提示词 | 参数 |
| --- | --- |
| `analyze_audio` | `sampleRate`，可选 `channels` |
| `change_tempo_safely` | 无 |
| `create_beat_or_song`、`sequence_advanced_drums`、`design_owned_sound`、`compare_reference_mix`、`diagnose_performance_setup` | `traits`，可选 `experienceLevel`（`beginner` 或 `advanced`）和 `bars`（`"1"` 到 `"16"`，以字符串形式） |

提示词和资源只做描述，不授权任何操作。引导流程提示词的说明见[操作示例](USER_JOURNEYS.md)。

## 环境变量

| 变量 | 作用 |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY`、`ABLETON_MCP_TOOL_ALLOW`、`ABLETON_MCP_TOOL_DENY` | [部署策略](#部署策略) |
| `ABLETON_MCP_EXTENSION`、`ABLETON_MCP_EXTENSION_DIR`、`ABLETON_MCP_LIVE_EXTENSIONS_DIR` | [Kumi 的 Live 扩展](#kumi-的-live-扩展) |
| `ABLETON_MCP_IMPORT_STAGING_DIR` | 导入的文件为 Live 复制到的位置（绝对路径；默认 `~/.config/ableton-mcp/import-staging`，Windows 上为 `%APPDATA%\ableton-mcp\import-staging`） |
| `ABLETON_MCP_USER_LIBRARY` | Live 的 User Library，用于加载到 Drum Sampler 的采样（桥接会在其 `Kumi` 文件夹中写入一个载体预设） |
| `ABLETON_MCP_LIVE_RESOURCES` | Live 的 Resources 文件夹，默认的 Drum Sampler 预设就在其中 |
