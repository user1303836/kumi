# 恢复步骤

[English](../en/RECOVERY.md) · 简体中文 · [日本語](../ja/RECOVERY.md)

调用失败或修改结果不确定时该怎么做。

原则：**永远不要用一项新的修改去应对一项不确定的修改。** 如果应用请求超时或丢失了应答，只能在服务器和 Live 都还在运行时，用相同的 `transactionId` 和 `idempotencyKey` 重试同一个应用请求。否则，请重新读取 Live，并手动把状态改正。

## 读懂错误

格式错误的请求会收到 JSON-RPC 错误：

| 代码 | 含义 |
| --- | --- |
| `-32700` | 这一行不是 JSON |
| `-32600` | 无效请求、消息超过 500 MiB、`id` 已被使用，或服务器正在关闭 |
| `-32601` | 没有这个工具 |
| `-32602` | 参数无效，或协议版本有误 |
| `-32002` | 尚未初始化，或没有这个资源或提示词（`2025-11-25`） |
| `-32022` | 不支持的协议版本（错误中会列出支持的版本） |
| `-32000` | 忙：等待中的请求太多；等一部分完成后再重试 |
| `-32603` | 内部错误 |

拒绝执行的工具会返回 `isError: true` 和 `{"reason": "...", "remediation": "..."}`。原因（reason）是桥接或 Live 自己给出的，只有一行。请阅读它：

- **“Nothing changed in Live”**，或以 “; nothing changed” 结尾的原因：这次调用什么也没做。按它说的修正，然后重新预览。
- **“Live state changed since the preview”**：预览和应用之间有东西变了，可能是你自己在 Live 中的编辑，也可能是其他客户端。重新读取，然后重新预览。
- **`tool-unavailable-in-current-live-shape`**：当前的 Live 此刻不提供该工具。读取 `ableton://capabilities` 查看原因。
- **`tool-denied-by-deployment-policy`**：[部署策略](USER_GUIDE.md#部署策略)隐藏了它。
- **remediation 说修改结果不确定**：见下一节。

## 修改结果不确定时

在应用或撤销过程中出现超时、断线或丢失应答，会让修改处于不确定状态：它可能已经发生，也可能没有。

1. 不要重新预览，也不要发送新的键。
2. 只要同一个服务器还在运行、Live 也没有重启，就用相同的 `transactionId` 和 `idempotencyKey` 再次发送同一个应用（或撤销）请求。Remote Script 记得自己执行过什么。它会返回第一次的结果，或者把修改完成，然后由桥接读回。
3. 如果 Live 在此期间重启过，重试会被拒绝。请读取工程（`live_discover`、`live_snapshot`），确认修改是否存在，并手动把状态改正。
4. 然后用 `live_recovery_finalize` 关闭这条记录：

   ```json
   {"transactionId": "<id>", "resolution": "manually-restored", "confirmation": "finalize-recovery-record",
    "evidence": {"provenance": "checked the mixer in Live", "scope": "track 3 volume"}}
   ```

   如果你决定保留 Live 现在的状态，请使用 `"accepted-current-state"`。关闭记录不会改变 Live 中的任何东西。只要有任何东西在播放、录音或占用实时通道，它就会被拒绝。

不确定的记录会占用服务器的撤销容量。当它们占满容量时，新的修改会被拒绝（“capacity is exhausted by recovery-protected work”），直到你关闭这些记录。

## 常见问题

| 问题 | 处理方法 |
| --- | --- |
| 由 Kumi 1.7.5 或更早版本安装的 JavaScript 桥接以 “Unsupported Node.js” 退出 | 用 Node 22 或 24 运行。原生桥接不使用 Node。 |
| “version-1 configuration does not enable a Live adapter” | 用 `ableton-mcp-setup` 加上桥接选项写出版本 2 的文件；见[配置文件](USER_GUIDE.md#配置文件)。 |
| “secret file is invalid”，或其权限 “must be conclusively owner-only” | 密钥必须是一行 32 个或更多字符，且只有你能读取。如果是通过生命周期工具安装的，`ableton-mcp-lifecycle repair` 会恢复正确的权限。 |
| `live_status` 显示 `"connected": false` | 确认 Live 正在运行，并已将 **AbletonMcpBridge** 选为控制界面（Control Surface），且配置中的端口和密钥与 Remote Script 使用的一致。然后运行 `ableton-mcp-diagnostics --config <path>`。 |
| AbletonMcpBridge 没有在 Live 中加载；Live 的日志显示 “bridge configuration reference is missing or unsafe” | Remote Script 安装时没有带 `--config`。请带上 `--config` 重新安装（见[连接到 Live](USER_GUIDE.md#连接到-live)），或使用生命周期工具。 |
| “Unknown or expired … transaction” | 预览已过期，或服务器已重启。请重新预览。 |
| `live_undo` 拒绝：引用 “isn't the one this change was made on any more” | 该对象已被替换。请手动改正；如果 Live 最后一个撤销步骤正是这项修改，也可以使用 `live_song_undo`。 |
| 你误删了东西 | 立即使用 `live_song_undo`（`confirmation: "undo-in-live"`）；`live_undo` 无法恢复删除。 |
| Live 扩展的工具不见了 | 扩展没有运行：它需要 Live 12.4 或更高版本，并且要么安装在 Live 的 Extensions 文件夹中，要么开启 Live 的 Developer Mode；见 [Kumi 的 Live 扩展](USER_GUIDE.md#kumi-的-live-扩展)。 |

安装问题（生命周期回执、隔离、修复和回滚）见[交付](DELIVERY.md)。

## 停止一切播放或录音

`live_session_emergency_stop` 会停止 Session 片段、走带以及两种录音模式。它不需要事务，所以重启后也能使用。

1. 读取正在播放的内容：用 `{"kind": "session-playback"}` 调用 `live_discover`。
2. 原样发送你读到的内容：

   ```json
   {"confirmation": "emergency-stop", "expectedTargets": ["<trackRef>|<clipSlotRef>|<sceneRef>"],
    "expectedRecording": "session"}
   ```

   `expectedRecording` 为 `stopped`、`session`、`arrangement` 或 `both`。

3. 如果因为播放状态在此期间发生变化而被拒绝，请重新读取并重复。成功时会报告 `"stopped": true` 和 `recordingStopped`。

要停止你触发的单个片段，请使用 `live_clip_launch_stop`；要停止试听，请使用 `live_session_audition_stop`。捕获有自己的紧急停止；见[音频智能](AUDIO_INTELLIGENCE.md)。对于实时控制，请调用 `live_realtime_disarm`，并参阅[实时控制](REALTIME_CONTROL.md)。

## 重启之后

服务器只在内存中保存撤销记录和预览，所以重启会丢失它们。Live 重启或重新连接会让 Live 获得新的 epoch，之前的所有引用都会失效。

1. 启动服务器并重新初始化。
2. 调用 `live_status`，并重新读取工程。
3. 重启前做出的修改无法用 `live_undo` 撤销。Live 自己的撤销（`live_song_undo`）可能仍然保留着它们。
4. 如果服务器停止时有修改处于不确定状态，请手动检查：已经没有可以重试或关闭的记录了。
