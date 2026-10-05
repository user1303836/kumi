# Ableton Live 安全

[English](../en/LIVE_SAFETY.md) · 简体中文 · [日本語](../ja/LIVE_SAFETY.md)

桥接在读取和修改你的 Live 工程时保证什么、不保证什么。桥接指 `crates/ableton-mcp-server` 中的 MCP 服务器，以及它在 Live 内运行的 Remote Script。Kumi 驱动它，任何 MCP 客户端也可以驱动它（[用户指南](USER_GUIDE.md)）。在让客户端操作一个你在乎的工程之前，请先读完本文。

## 信任边界

- 没有 `--config` 时，桥接使用它的不可用适配器：它从不读取或修改 Live。客户端发送的任何内容都无法选择别的适配器。
- 有配置时，桥接只通过回环地址（`127.0.0.1` 或 `::1`）连接 Live，并用一个仅所有者可读、至少 32 个字符的密钥为每个请求签名。它信任你的用户账户：以你的身份运行的程序可以读取这个密钥并驱动 Live。网络上的其他任何东西都做不到。
- 部署策略决定客户端能看到和调用哪些工具：`read-only`、`edit-no-audio`、`performance` 或 `full`（默认），并可用 `ABLETON_MCP_TOOL_ALLOW` 和 `ABLETON_MCP_TOOL_DENY` 进一步收窄。桥接在每次调用时都会检查它，包括应用、撤销和紧急停止。只有 `full` 包含 `live_run_python`；`ABLETON_MCP_TOOL_DENY=live_run_python` 可以关闭它。各个配置档见[用户指南](USER_GUIDE.md)。
- Kumi 以 `full` 启动桥接，并附带一份恰好列出它所用工具的允许列表。预览和应用由 Kumi 自己的代码执行，它们的确认也由这段代码保管；模型按名称请求修改。模型还可以在 Live 中运行 Python（见 [Live 中的 Python](#live-中的-python)）。

## 修改如何到达 Live

大多数修改都是先预览，再应用：

1. `*_preview` 工具读取最新状态，检查请求，并返回一个说明将要修改什么的事务。它不修改任何东西，10 分钟后过期。
2. `*_apply` 工具接收事务 id、它的确认（大多数工具是字面值 `apply`；场景试听、片段启动和音频捕获则是一个无法预测的令牌）以及一个幂等键。
3. 桥接把修改作为一个请求发给 Remote Script。在 Live 的主线程上，Remote Script 检查修改所携带的栅栏（它所指对象的身份及其所在位置：设备、设备所在的轨道及其同级设备；片段、片段所在的槽位及其场景），应用修改，并把结果记录在该幂等键下。然后桥接回读结果。

`live_change` 在一次调用中完成预览及其应用。对于需要你先看到预览再做决定的修改，它不会这样做：试听、片段启动、捕获、录音、实时布防以及 Live 的对话框。

对于调音台、链、路由、片段、场景、轨道和歌曲设置、调律、视图、重命名、参数和设备编辑、Simpler 采样、Browser 加载、数据和删除，预览还会获取一份摘要，记录这项修改所依赖的内容：修改所指的各行、歌曲的走带状态，以及（参数、调音台和重命名修改除外）工程的轨道与场景结构。应用时会携带这份摘要，只要其中任何一项不同，Remote Script 就会以 "Live state changed since the preview" 拒绝。摘要只覆盖事务的第一步，有效期 10 分钟。依赖正在播放内容的修改（走带、启动、捕获、录音）不携带摘要，因为在预览和应用之间播放会继续推进。

一项修改的结局：

- **被拒绝，什么都没改。** 以 "nothing changed" 结尾的拒绝，发生在 Live 中执行任何操作之前。按原因所说修正后，重新预览。
- **已应用。** 用同一个幂等键重试，会返回已记录的结果，而不会再次应用。Remote Script 运行期间会保留这些结果；显式重连会清除它们。
- **不确定。** 超时、回复丢失或回读失败，都会让修改处于不确定状态。不要用新的预览或新的键重试。在同一个 Live 会话仍然连接时，用同一个键重试同一个应用，它会返回实际发生了什么；或者检查 Live 并手动恢复（[恢复](RECOVERY.md)）。如果同键重试被拒绝，能确定的只是这次重试没有执行，所以这项修改仍然是不确定的。

引用属于同一个 Live 会话（一个 epoch）。Live 重启或桥接重连之后，之前的引用、事务和撤销都不再适用。

每项修改还遵循以下规则：

- **数值适配参数。** 超出参数范围的值会取最近的端点，落在分级参数两级之间的值会取最近的一级。已关闭设备上的旋钮，以及 Live 不对其自动化的参数，都可以设置；只有 Live 显示为灰色的参数会被拒绝。实时平面更严格（见[实时控制](REALTIME_CONTROL.md)）。
- **名称可以重复。** 轨道和场景可以同名；修改按身份而不是按名称查找对象。
- **读取保持简短。** 一个发现分页或快照窗口在 Live 线程上工作约 30 ms 后就会停下，并为剩余部分返回一个游标，所以无论工程多大，任何读取都不会卡住 Live 的界面。

## 撤销

- `live_undo` 撤回一个已应用的事务，仅限同一个 Live 会话内。
- 撤销跟随对象，而不是对象的内容。修改所创建的轨道、场景、返回轨、副本、设备或片段，即使之后被重命名、或加入了片段或设备，也会被移除；参数、推子或速度会回到修改之前的值，无论此后它怎样变化。如果引用现在指向另一个对象，或对象已经不存在，撤销会被拒绝。
- 有两个例外跟随的是内容。由 `live_midi_clip_apply` 创建的 Session MIDI 片段，只有在其名称、长度和音符仍保持修改后的样子时才会被移除。通过 Kumi 的 Live 扩展写入的编曲视图 MIDI 片段，一旦其音符、名称或范围发生变化就会保留下来。如果这样的片段应该去掉，请自己删除它。
- 如果撤销在触及 Live 之前就被它的检查拒绝，修改会保持已应用，下一次撤销会重新检查。
- 删除和瞬时动作（[事务模型之外](#事务模型之外)中列出的那些，以及机架、Looper 和部分视图操作）没有事务撤销，`live_undo` 会这样告诉你。Live 自己的 Cmd-Z 仍然能撤回删除。删除工具（`live_clip_delete_*`、`live_scene_delete_*`、`live_track_delete_*`、`live_locator_delete_*`、`live_device_delete_*`）按身份为对象及其所在位置设置栅栏，因此绝不会误删顶替它的其他对象。
- MIDI 片段、批量和设备状态事务各自最多保留 512 条记录。超出之后，最早的已应用事务将无法再撤销。

一个计划可以成为 Live 自身撤销中的一步：`live_undo_step_begin` 打开一个步骤，`live_undo_step_end` 关闭它，两者之间 Remote Script 所做的一切修改只需一次 Cmd-Z 即可撤回。打开的步骤归 Remote Script 所有：打开它的连接关闭时、时间用尽时、重连时以及退出时，Remote Script 都会关闭它。步骤打开期间，你自己在 Live 中的编辑会自成一步。通过 Live 扩展进行的修改不会归入其中。`live_song_undo` 和 `live_song_redo` 按下的是 Live 自己的撤销和重做，针对在 Live 中所做的操作；它们不是事务的撤销。

## 事务模型之外

这些工具没有事务撤销。允许它们之前，先弄清每个工具会做什么。

| 工具 | 作用 | 检查什么，以及如何结束 |
| --- | --- | --- |
| `live_run_python` | 在 Live 中运行 Python（见下文） | 事先不做任何检查；一个 Live 撤销步骤 |
| `live_transport_action_preview/apply` | 开始、继续、播放选区、拖动播放、敲击速度、微调、跳转、触发 Session 录音等 | 以工程及其播放状态为栅栏。`trigger-session-record` 会开始 Session 录音，不经过下文的录音检查。 |
| `live_scene_fire_preview/apply` | 像 Live 启动所选场景那样触发一个场景 | 以该场景、场景列表，以及它是否已被触发或走带是否在播放为栅栏；空场景会被拒绝 |
| `live_fire_button_preview/apply` | 按下或松开片段、槽位或场景的启动按钮 | 一直按住，直到松开、连接关闭或 30 s 后 |
| `live_browser_preview` | 播放 Browser 条目的预听 | 用 `live_browser_preview_stop` 加上它的 `previewId` 停止 |
| `live_message` | 在 Live 的状态栏或对话框中显示一条消息 | 不修改工程中的任何东西 |
| `live_song_undo`、`live_song_redo` | Live 自己的撤销和重做 | 取决于 Live 的历史记录中有什么 |

### Live 中的 Python

`live_run_python` 在 Live 的主线程上运行 Python，处理那些类型化工具覆盖不到的事情。脚本运行期间，Live 会等待。

- 参数：`code`（最多 64 KiB）、`mode`（`exec` 为默认值，返回代码赋给 `result` 的值；`eval` 返回一个表达式的值）、`timeoutMs`（1–30,000，默认 5,000）以及可选的 `ref`。
- 代码可用的名称：`Live`、`song`、`app`、`obj`（`ref` 所指的对象）和 `bridge`（Remote Script 的对象映射器，连同它的全部内部实现）。
- 它返回 `{ok, result, stdout, error}`。结果中的 Live 对象以 `{ref, type, name}` 的形式返回，其中的 ref 可被其他工具接受。错误带有类型、消息和回溯（traceback）。
- 每次运行是一个 Live 撤销步骤（"Kumi: Python"），或并入一个已经打开的步骤，所以在 Live 中按 Cmd-Z 就能撤回它。
- 没有预览、摘要、幂等键、事务撤销，也没有 HISTORY 条目。超时或回复丢失之后，脚本做了什么是未知的：请重新读取工程。
- 超时会中断 Python 代码；对 Live 的一次耗时调用，只有在它返回时才会被检查。
- 脚本可以做 Live 的 Python API 允许的任何事，包括播放、录音和删除。如果某个客户端不应拥有这个工具，请通过部署策略拒绝它。

Kumi 以 `run_python` 的名称把它提供给模型，并在每次运行后忘掉它持有的所有 Live 引用。

## 可发声与录音操作

走带操作、场景触发和启动按钮见上表。除场景试听外，每个工具的输出安全证据（`outputSafety`）都是可选的：客户端没有提供时，桥接会提供它自己的证据。桥接听不到你的音箱，所以请把监听音量保持在安全水平。

| 操作 | 工具 | 运行前检查 | 如何停止 |
| --- | --- | --- | --- |
| 片段启动 | `live_clip_launch_preview/apply` | 预览中的轨道、场景、槽位和片段身份。无论当时有什么在播放或录音，它都会启动，就像在 Live 中按下该槽位一样。 | `live_clip_launch_stop` 停止该片段所在的轨道，仅当该片段是这条轨道唯一正在播放的目标时 |
| 场景试听 | `live_session_audition_preview/apply/stop` | 工程的确切名称；走带已停止，没有任何内容在录音或播放，没有预备录音或监听输入的轨道，启动量化不是 None；调用方的输出安全证据。在 Live 线程上，它会重新检查工程、场景、播放修订号和每个目标。 | `live_session_audition_stop`，只停止它自己的目标 |
| 录音 | `live_recording_preview/apply` | 一条已预备录音的目标轨道（`destinationTrackRef`），以及最多 1,024 条与它一起录音的其他已预备录音轨道（`alsoTrackRefs`）。其他轨道也可以处于预备录音状态；Live 会在每条已预备录音的轨道上录音。Session 和 Arrangement 的录音状态以及目标轨道的身份会在 Live 线程上重新检查。 | 以 `stop` 操作预览并应用，或紧急停止 |
| 音频捕获 | `live_audio_capture_*` | 见[音频智能](AUDIO_INTELLIGENCE.md#live-捕获) | Live 中的 10 秒看门狗，以及它自己的紧急停止 |
| 实时 | `live_realtime_*` | 见[实时控制](REALTIME_CONTROL.md) | `live_realtime_disarm` |

`live_session_emergency_stop` 一次性停止 Session 片段、走带、Session Record 和 Arrangement Record。它从最新的播放发现中获取活动目标和录音模式（`stopped`、`session`、`arrangement` 或 `both`），如果此后 Live 的状态已经变化，它会拒绝。它不属于任何事务，所以在宿主重启之后仍然有效。

没有任何只读工具会开始播放或录音。

## 提供给 Live 的文件

当工具加载音频文件时（音频导入、项目导入、Simpler、鼓垫、采样加载），桥接会把经过验证的文件以只读方式复制到一个受管理的文件夹，只把这份副本提供给 Live：macOS 和 Linux 上是 `~/.config/ableton-mcp/import-staging`，Windows 上是 `%APPDATA%\ableton-mcp\import-staging`，或者 `ABLETON_MCP_IMPORT_STAGING_DIR` 中的绝对路径。这个文件夹仅所有者可访问。在检查和加载之间，文件无法被调包。

Live 从导入音频所在的位置播放它，所以应用成功之后，这份副本就是该片段或 Simpler 的采样，直到你收集工程的文件或删除该片段为止。只有在没有任何东西使用副本时（应用失败或被拒绝、事务过期或已完结、片段已不存在后的撤销），桥接才会删除它，绝不会在成功时或退出时删除。用到旧副本的片段都不在了之后，请手动删除这些旧副本。

## 桥接与 Remote Script

发给 Remote Script 的每个请求都经过 HMAC 签名，并绑定到一个 Remote Script 会话和一个连接，带有严格递增的序号和一个截止时间。Live 的主线程读取套接字并亲自执行每个请求，每次显示刷新执行一部分，因此没有任何东西会从其他线程触碰 Live。在 Windows 上，“仅所有者”指一份只含你一个条目的访问控制列表。如果你在安装时提出要求，Remote Script 可以写一个小的诊断文件。详情见 `remote-script/README.md`：[Remote Script 的 README](../../remote-script/README.md)。

## 在真实 Live 上验证过什么

测试、模拟器和假 Live 映射器检查的是桥接的契约，而不是 Live 的行为。真实 Live 的证据位于 `docs/evidence`，每个文件都注明了日期、Live 版本和桥接版本，并在[实现状态](IMPLEMENTATION_STATUS.md)中编入索引。最新的运行是在 macOS 上的 Live 12.4.15 测试版上进行的。2026 年 7 月的阶段文件（Live 12.4.5b8，桥接 0.1.0）早于当前的桥接。没有记录任何 Windows 上的 Live 运行。

如果 Live 显示的内容与桥接报告的不一致，请停止客户端，保留证据，如果有内容在播放就使用紧急停止，并把它当作一个 bug 处理。
