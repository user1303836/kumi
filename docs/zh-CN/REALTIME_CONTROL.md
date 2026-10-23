# 实时控制

[English](../en/REALTIME_CONTROL.md) · 简体中文 · [日本語](../ja/REALTIME_CONTROL.md)

Remote Script 中的一个 UDP 端点，供控制器脚本、OSC 应用或 Max patch 快速控制参数，每次最长 30 秒。它是为 MCP 客户端准备的；Kumi 不使用它。

开放的端口本身不授予任何权限。在客户端通过桥接布防这个平面之前，所有数据包都会被丢弃，而一次布防会指明数据包可以改动的确切参数。

## 配置

当 Remote Script 版本 2 配置的 `bridge` 部分中有一个不同于 `port` 的 `realtimePort` 时，Remote Script 会监听 UDP：

```json
"bridge": {"host": "127.0.0.1", "port": 9765, "realtimePort": 9766, "secretFile": "/absolute/path/to/bridge.secret", "timeoutMs": 5000}
```

完整的配置文件在 Remote Script 的 README 中说明（[daws/ableton/remote-script/README.md](../../daws/ableton/remote-script/README.md)）。

- `ableton-mcp-lifecycle install`（因此也包括 `kumi bridge`）会把 `realtimePort` 设为 9766，除非 `--realtime-port` 指定了别的端口。安装前会先检查两个端口是否都空闲。
- `ableton-mcp-setup` 只有在你随其他桥接选项一起传入 `--realtime-port` 时才会写入它（[用户指南](USER_GUIDE.md)）。
- 没有 `realtimePort` 时，不会有 UDP 套接字，也不会提供 `realtime.*` 操作。

两个套接字都只绑定配置的回环地址。如果 Live 启动时 UDP 端口已被占用，Remote Script 就完全不会加载，TCP 也一样：请释放该端口，或换一个端口重新安装。

## 布防

1. 调用 `live_realtime_arm_preview`，参数为：
   - `channels`：`udp-json`、`osc`、`xy` 和 `max` 中的一到四个；
   - `parameterRefs`：来自最新一次发现的 0 到 32 个参数引用（空列表只允许紧急停止数据包）；
   - `ttlMs`（可选）：1,000 到 30,000，默认 10,000；
   - `sourcePorts`（可选）：最多 16 个允许数据包来源的 UDP 端口；
   - `outputSafety`（可选）。

   它需要一个已连接、并提供 `realtime.arm`、`realtime.disarm` 和 `realtime.stats` 的 `real-live` Remote Script。预览会记录每个参数的身份，连同它的设备、它的轨道以及该设备的其他参数。
2. 调用 `live_realtime_arm_apply`，传入事务 id、`confirmation: "apply"` 和一个幂等键。在 Live 的线程上，Remote Script 会把记录下的每个身份与 Live 当前的状态对比，如果某个参数被替换或移动了就拒绝。
3. 结果包含端点（`host`、`port`）、一个 bearer `token`、`expiresAt`、通道和参数引用，以及各项限制。不要让令牌出现在日志和文件中。
4. 发送数据包（见下文），并读取 `live_realtime_stats` 查看它们的结果。
5. 完成后，用 `confirmation: "disarm"` 调用 `live_realtime_disarm`。

再次布防会替换令牌、让序号重新开始，并丢弃先前那次布防仍在队列中的写入。撤防、过期和 Remote Script 关闭也会丢弃它们。

## 数据包

### UDP JSON

每个数据报一个 JSON 对象，最多 512 字节。未知字段会被拒绝。

设置一个参数：

```json
{"token":"<arm token>","seq":1,"channel":"udp-json","op":"parameter.set","ref":"<parameter ref>","value":0.5,"sentAtMs":1700000000000}
```

同时设置两个参数（只要任一写入失败，两者都会恢复原值）：

```json
{"token":"<arm token>","seq":2,"channel":"xy","op":"xy.set","xRef":"<parameter ref>","x":0.4,"yRef":"<parameter ref>","y":0.6,"sentAtMs":1700000000000}
```

紧急停止：

```json
{"token":"<arm token>","seq":3,"channel":"udp-json","op":"emergency-stop","sentAtMs":1700000000000}
```

- `seq` 是不超过 2^53 − 1 的正整数，在同一次布防中每个数据包都要递增。更小或重复的 `seq` 视为重放，会被丢弃。
- `sentAtMs`（可选，自 1970 年起的毫秒数）让统计可以测量传输抖动；没有它时，统计测量的是到达间隔的抖动。
- `channel` 必须是已布防的通道之一，并且必须适用于该操作：

| 操作 | 通道 |
| --- | --- |
| `parameter.set` | `udp-json`、`osc`、`max` |
| `xy.set` | `xy`、`osc`、`max` |
| `emergency-stop` | 任何已布防的通道 |

### OSC

OSC bundle 和不支持的参数类型会被拒绝。

| 地址 | 参数 |
| --- | --- |
| `/ableton-mcp/parameter` | token（字符串）、seq（int32 或 int64）、ref（字符串）、value（数字）、可选的 sentAtMs（数字） |
| `/ableton-mcp/xy` | token、seq、xRef、x、yRef、y、可选的 sentAtMs |
| `/ableton-mcp/emergency-stop` | token、seq、可选的 sentAtMs |

数字可以是 int32、int64、float32 或 float64。

### Max

Max patch 可以通过 `udpsend` 把带有 `"channel": "max"` 的 UDP JSON 对象发送到返回的端点。桥接不附带任何 Max 设备，也没有 Max 握手。桥接的 `ableton://max-extension` 资源描述了数据包约定。

### 紧急停止

`emergency-stop` 数据包会在 Live 的线程上停止当时正在播放的内容：Session 片段、走带、Session Record 和 Arrangement Record。`live_session_emergency_stop` 通过经过认证的 TCP 通道做同样的事，不需要令牌。

## 限制

| 项目 | 限制 |
| --- | --- |
| 数据包大小 | 512 字节 |
| 速率 | 持续每秒 64 个数据包，突发 16 个（令牌桶） |
| 每次布防的参数数 | 32 |
| 每次布防的发送端口数 | 16 |
| 布防有效期 | 1 到 30 秒 |
| 等待 Live 线程 | 1 秒；届时仍未开始的写入会被丢弃 |
| 队列 | 与 Remote Script 的主线程队列共享（65,536 个条目） |

## 数据包的处理过程

在从不触碰 Live 的 UDP 线程上，Remote Script 解码数据包，检查令牌、发送方、通道、目标参数、序号和速率，然后把写入排入 Live 主线程的队列。因速率被丢弃的数据包已经用掉了它的 `seq`：请发送下一个序号，而不是再发同一个。

在 Live 的主线程上，下一次显示刷新时，它检查这次布防是否仍然有效，以及每个参数是否仍具有布防时记录的身份；任何变化都会撤销这次布防。然后它检查数值：在参数范围内、落在参数的某一级上，并且参数处于启用状态。不合格的值会被拒绝，绝不会被调整到合适的值（这一点与类型化的参数工具不同）。之后它写入该值并回读。

`accepted` 表示已排入 Live 线程的队列，而不是已写入；UDP 本身从不确认送达。只有 `applied` 表示已写入并回读。

`live_realtime_stats` 报告：

| 字段 | 含义 |
| --- | --- |
| `armed` | 当前是否处于布防状态 |
| `accepted`、`applied`、`pending` | 已排队、已写入并确认、仍在等待 |
| `applyFailures`、`revokedBeforeApply` | 在 Live 线程上失败的写入，以及因布防结束或目标变化而被丢弃的写入 |
| `droppedBeforeDispatch`、`droppedQueueFull` | Live 线程没有在 1 秒内开始的写入，或无法排队的写入 |
| `droppedUnarmed`、`droppedEndpoint`、`droppedTarget`、`droppedInvalid`、`droppedReplay`、`droppedRateLimited` | 被丢弃的数据包：未布防或令牌、通道错误；发送方不对；参数未布防；格式错误；重放；超出速率 |
| `sequenceGaps`、`lastSequence` | 至今缺失的序号数，以及最后接受的序号 |
| `jitterMs`、`maxJitterMs` | 平滑后的抖动和最大抖动 |

## 恢复

- 一旦出现任何 `applyFailures`、`revokedBeforeApply` 或 `droppedBeforeDispatch`，或者 `pending` 降不到零，请先撤防并重新发现，再重新布防。
- 被丢弃的数据包只会被计数，绝不会替你重试。
- 如果 MCP 宿主重启而 Live 继续运行，令牌在过期之前仍然有效；重新连接不会延长它。
- 如果 Live 或 Remote Script 重启，套接字会关闭，所有令牌都会失效。
- 当一次布防仍然有效或有写入待处理时，`live_recovery_finalize` 会拒绝。
- 一次会话结束后，把你改动过的参数恢复原样，并检查 Live 已经停止且没有在录音。

## 证据

[phase-7c-realtime-live.json](../evidence/phase-7c-realtime-live.json) 在 macOS 上的 Live 12.4.5b8 上（2026-07-27，桥接 0.1.0）测试了全部四个通道、重放、发送方和目标的丢弃，以及参数恢复。它早于当前的桥接，也没有记录任何 Windows 上的运行。没有测试过任何 Max 设备。
