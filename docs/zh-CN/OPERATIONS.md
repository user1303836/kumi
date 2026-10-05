# 运维指南

[English](../en/OPERATIONS.md) · 简体中文 · [日本語](../ja/OPERATIONS.md)

日常运行桥接：启动、检查、限制，以及它写入磁盘的内容。安装方法见[交付](DELIVERY.md)。出现故障时，见[恢复](RECOVERY.md)。

## 启动

由 MCP 客户端启动服务器，每个客户端一个进程：

```sh
/absolute/path/ableton-mcp-server --config /absolute/path/bridge-config.json
```

服务器从 stdin 读取 JSON 行形式的 MCP 消息，并把消息写到 stdout。stdout 只能用于 MCP。服务器自己的日志行写到 stderr，带 `mcp-host:` 前缀。请在服务器的环境中设置[部署策略](USER_GUIDE.md#部署策略)和其他[环境变量](USER_GUIDE.md#环境变量)。

Kumi 会启动放在它旁边的原生服务器，把策略设为自己用到的那些工具，并最多等待 65 秒的应答。分析工作进程留在服务器旁边。

## 检查连接

调用 `live_status`。连接正常时会显示：

- `"connected": true`；
- `"adapter": "remote-script"`；
- `"provenance": "real-live"`；
- 数字形式的 `epoch`；
- Remote Script 提供的注册表哈希和操作。

不要把端口开着或 Live 正在运行当作连接正常的证明：只有经过认证的 `live_status` 才算。

`ableton-mcp-server diagnostics --config <path>` 在终端中做同样的检查。它报告原生运行时、软件包、配置和密钥的权限，然后对工程进行一次简短的认证读取（工程、场景、轨道、播放状态、一条轨道的片段槽）。[交付](DELIVERY.md)逐阶段解释了这份报告。

## 限制

| 限制 | 值 |
| --- | --- |
| 单条 MCP 消息 | 500 MiB |
| 来自 Remote Script 的单条消息 | 256 MiB |
| 等待 Remote Script 处理的请求 | 4,096 |
| 正在处理的请求 | 同时 16 个；等待数超过 64 时，新请求会收到 `-32000 Server is busy` |
| 发往 Live 的请求的截止时间 | `timeoutMs`（默认 5 s）。快照和发现获得其六倍的时间，预览和应用获得 15–45 s，外加工程中每条轨道 20 ms。从不超过 60 s。 |
| 预览有效期 | 10 分钟；批量、MIDI 片段和设备状态预览 30 s；捕获预览 60 s |
| 保留的撤销记录 | 总计 1 GiB；批量、MIDI 片段和设备状态各 512 条（最早应用的修改先放弃其撤销） |
| 为慢速客户端排队的事件 | 65,536 个，之后发送 `notifications/live_event_overflow` |
| 每页音符数 | 2,000 |
| 单次修改中的参数数 | 10,000 |
| 批量操作 | 32 个操作 |
| 音频分析 | 10,000,000 个采样或 600 s；同时 2 个工作进程，4 个等待，每个 30 s |
| 参考比较 | 4,000,000 个采样，每个音源 30 s，对齐偏移 10 s |

工具调用没有频率限制。[实时控制](REALTIME_CONTROL.md)和[音频智能](AUDIO_INTELLIGENCE.md)列出了各自的限制。

## 应答、取消与关闭

每个请求一完成就立即发出应答，所以慢请求（导出大工程、渲染）不会拖住其他请求。请按 `id` 把应答与请求对应起来。

`notifications/cancelled` 可以停止尚未开始的请求。如果请求已经到达 Live，取消并不会撤销它。修改可能已经发生，所以在做其他修改之前先读取一次。

关闭 stdin 会结束服务器。它会先关闭仍处于打开状态的撤销步骤，再断开与 Live 的连接。重启后的服务器没有任何撤销记录；见[恢复](RECOVERY.md#重启之后)。

## 桥接写入的文件

| 内容 | 位置 |
| --- | --- |
| 配置、密钥、回执、操作日志 | 生命周期工具的状态文件夹：`~/.config/ableton-mcp`，Windows 上为 `%APPDATA%\ableton-mcp`，除非你另选了位置 |
| Remote Script 诊断日志 | 状态文件夹中的 `bridge-diagnostics.log`，仅在 `ableton-mcp-server lifecycle install --enable-bridge-diagnostics` 之后生成。它只记录事件代码，不含名称或数据；上限 16 MiB，达到后从头开始。 |
| 导入音频的副本 | `~/.config/ableton-mcp/import-staging`（`%APPDATA%\ableton-mcp\import-staging`），或 `ABLETON_MCP_IMPORT_STAGING_DIR`。Live 播放的是这些副本，所以只有在没有片段再使用它们时才能删除；见 [Live 安全](LIVE_SAFETY.md)。 |
| Drum Sampler 载体预设 | Live 的 User Library 中的 `Kumi` 文件夹，加载后即删除 |
| 设备状态 | 你传给 `live_device_state_save` 的文件夹 |
| 工程备份 | 已保存工程的旁边（`live_project_backup_apply`） |
| 渲染文件 | Live 扩展的临时文件夹，保留 6 小时 |

## 多个客户端

每个客户端启动自己的服务器，Remote Script 最多接受 64 个连接。各服务器之间不共享撤销记录，也不知道彼此做的修改。应用请求因 “Live state changed since the preview” 被拒绝，可能是其他客户端造成的：请重新读取，然后重新预览。
