# 音频智能

[English](../en/AUDIO_INTELLIGENCE.md) · 简体中文 · [日本語](../ja/AUDIO_INTELLIGENCE.md)

桥接为 MCP 客户端提供的音频工具。它们测量你发送的音频，将其与参考音频对比，把测量结果与 Live 中的一条轨道关联起来，并从 Live 录制一段简短的捕获，测量后删除。

**Kumi 不使用这些工具。** Kumi 有自己的聆听能力，在你的电脑上运行：它读取音频文件和工程中的音频片段，录制或渲染它搭建的内容（它的 `audition` 工具通过 Kumi Ears 设备在 Live 中聆听每个候选，没有 Max for Live 时则把每个候选录到一条临时轨道上，期间 Main 按你的设置照常播放；`render` 通过 Kumi 的 Live 扩展渲染一条音频轨道的片段），然后测量结果。见 Kumi 指南中的[聆听](KUMI_GUIDE.md#聆听)，以及 [Kumi 如何修改你的工程](KUMI_CHANGES.md)。

## 工具

| 工具 | 输入 | 输出 |
| --- | --- | --- |
| `audio_analyze` | base64 编码的交错小端 float32 PCM，归一化到 −1…1：8–384 kHz，1–32 个声道，最多 10,000,000 个采样和 600 秒 | `pcm-analysis/v3`：响度、峰值、频谱、动态、瞬态、削波 |
| `audio_compare_reference` | 两个这样的音源：32–96 kHz，单声道或立体声，每个最长 30 秒，合计最多 4,000,000 个采样 | `reference-analysis/v2`：两者的分析、对齐结果以及差异 |
| `audio_diagnose_live_context` | 单声道或立体声 PCM（与 `audio_analyze` 相同），外加一个 `trackRef` | 分析结果，其中的发现与该轨道、它的设备和路由相关联 |
| `live_audio_capture_preview/apply`、`live_audio_capture_status`、`live_audio_capture_emergency_stop` | 一个 Session 片段、一个空的音频槽位和 1–9 秒 | 对 Live 的 Resampling 输入的分析；录音随后会被删除 |

这些工具都不接受文件路径或 URL，也都不返回音频。你发送的 PCM 绝不会被当作 Live 的输出：与 Live 的关联始终标记为需要由你担保，只有桥接自己进行的捕获例外。

## 测量

`pcm-analysis/v3` 中的 `standardsAudio` 遵循已发布的标准：

- ITU-R BS.1770-5 节目响度，采用 EBU R128 的操作规范；
- EBU Tech 3341 瞬时（400 ms）和短期（3 s）响度；
- EBU Tech 3342 响度范围：−70 LUFS 绝对门限，−20 LU 相对门限；
- 综合响度取自每 100 ms 一个的 400 ms 块，以 −70 LUFS 和 −10 LU 为门限；
- 声道权重：单声道和立体声会自动推断，更多声道的布局需要标签 `M`、`L`、`R`、`C`、`Ls`、`Rs` 和 `LFE`；LFE 不计入，环绕声道的权重为 1.41；
- 采样峰值和真峰值，分别报告。

真峰值在 48 kHz 下使用 BS.1770-5 附录 2 的四相 48 阶滤波器；在 44.1 kHz 下，先用 64 抽头 Blackman 窗 sinc 转换到 48 kHz。在其他采样率下，真峰值报告为不可用。响度适用于 8 到 384 kHz 之间的任何整数采样率，在 48 kHz 下使用已发布的滤波器系数。

静音、过短的音频、未知的多声道布局以及超出真峰值计算上限的输入，都会给出明确的不可用值，绝不会是 NaN、无穷大或编造的数字。瞬时和短期序列最多保留 128 个点；门限计算仍然使用每一个窗口。

`clipping` 统计达到满刻度的源采样。`reconstructedOvers` 统计只在重建之后才出现的 0 dBFS 以上的值；它们不能证明源发生了削波。`loudness` 字段是为兼容而保留的旧版 RMS 估计值：交付和母带决策请使用 `standardsAudio`。

## 参考对比

`audio_compare_reference`：

1. 用 32 抽头 Blackman 窗 sinc 内核把每个音源转换到 48 kHz；
2. 先在 100 Hz 下粗搜索，再在 ±10 ms 范围内以 1 kHz 精搜索，从而对齐它们（`alignment.mode` 为 `auto`，即默认值；或者带偏移量的 `manual`，或 `disabled`）。`maxLagSeconds` 默认为 5，最大为 10；
3. 拒绝较弱、静音或有歧义的自动匹配：每个音源仍会被分析，但重叠为零，所有差异都不予给出；
4. 否则只分析重叠部分，并报告综合响度的差异、±24 dB 以内的电平匹配建议，以及真峰值和采样峰值、RMS、峰值因数、动态范围、频谱和瞬态密度的差异。

`resampling.*.sourceClipping` 统计每个音源在转换前的满刻度采样。重采样不会拉伸时间或匹配速度，建议的电平匹配也不会改变任何音频。不会返回对齐后的音频。

## 诊断

`audio_diagnose_live_context` 分析你的 PCM，读取一条轨道的一份最新快照（工程、该轨道、它的调音台和路由、按顺序排列的设备及其参数值），并把测量结果与之关联。这种关联被标记为由你声明且未经验证；进行过一次捕获之后，它会被标记为 `verified-by-capture-lifecycle`。

发现会把测量与假设分开。轨道上的设备绝不会被称为原因（`causality.claimed` 始终为 false），无法得知的事情会被明确指出：延迟、侧链、隐藏参数、增益衰减，以及信号当时位于设备内部的哪个位置。建议的调音台修改是一个可撤回的实验，供你尝试后再捕获一次，而不是一个承诺好的 dB 校正。

## 工作进程

分析从不在宿主自己的事件循环上运行。每个任务都在一个用完即弃的工作进程 `ableton-mcp-analysis-worker` 中运行：

| 项目 | 限制 |
| --- | --- |
| 任务 | 同时 2 个，排队 4 个 |
| 时间 | 30 秒 |
| 请求 | 64 MiB |
| 输出 | 2 MiB 结果，16 KiB 错误文本 |

工作进程不继承任何密钥，只以 JSON 形式返回结果。取消 MCP 请求或超时会立即终止它。

## Live 捕获

Live 的脚本接口无法访问它的音频，所以桥接改为录音：它把一个 Session 片段播放到 Live 的 Resampling 输入，录进一个空的音频槽位，测量这个文件，然后删除它。它只在真实 Live 上运行，并且要求 Remote Script 提供 `audio.capture.resampling` 以及全部六个 `audio.capture.*` 操作。在部署策略中，预览、应用和紧急停止属于 `capture` 类（只有 `full` 配置档包含它）；`live_audio_capture_status` 属于读取。

### 预览

`live_audio_capture_preview` 需要：

- 当前打开的工程的确切名称，该工程必须已保存；
- 一个要播放的 Session 片段，以及另一条轨道上一个不同的、用于录音的空音频槽位；
- 目标轨道当前可用的输入路由，以便之后恢复。如果 Live 显示的是过时的 `Ext. In`，请先用 `live_routing_*` 选择 `No Input`；
- 走带已停止，没有任何内容在录音或播放，所有轨道都未预备录音，也没有轨道在监听输入；
- `durationSeconds` 为 1 到 9；
- `consent: "ephemeral-analysis-and-delete"`；
- `outputSafety` 是可选的。

预览在 60 秒后过期。

### 应用

`live_audio_capture_apply` 接收预览给出的不可预测的确认和一个幂等键。在 Live 的主线程上，Remote Script 再次检查源和目标，用一个私有标记暂时重命名目标轨道，把它的输入设为 Resampling、关闭监听并打开预备录音，把启动量化设为无，然后触发两个槽位。只有当 Live 将新片段标记为正在录音、且其名称带有该标记时，它才认定这个新片段归自己所有；随后它恢复轨道名称和启动量化。它从不重试开始。

Live 中的看门狗最多在 10 秒后（请求的时长加 3 秒）停止捕获。手动停止、取消、看门狗、紧急停止和退出，都会停止两个槽位、走带和录音，并恢复播放头、轨道名称、路由、预备录音和监听。期间别人所做的修改会被报告，而不会被覆盖。

### 录音

宿主只接受新近创建的、普通的、单链接的 WAV，且它必须位于已保存工程的项目文件夹或 User Library 的 `Samples/Recorded` 中：最多 32 MiB、12 秒、两个声道，格式为 16、24 或 32 位 PCM 或 32 位浮点。读取文件时，它以文件的身份、大小、时间和 SHA-256 为栅栏。

分析之后，它在不跟随链接的情况下打开 WAV 及其 `.asd`，再次检查它们的身份，把它们移入同一磁盘上的私有隔离文件夹，清空并删除它们，然后才在 Live 中删除片段。之后它会检查没有留下任何媒体文件和隔离文件。“删除”指解除链接；它不保证数据已从 SSD 或写时复制磁盘上消失。成功的结果会显示：Live 已停止且未在录音、目标轨道已恢复、槽位为空、捕获状态为 `cleaned`，并且没有遗留文件。

结果包含格式、采样率、时长和分析，但不包含路径、摘要、音频、令牌或确认。

如果 Live 或 Remote Script 在捕获中途关闭，Remote Script 会停止并恢复它能恢复的内容，但无法自己删除文件：它会把片段及其文件留给宿主或你来清理。

### 恢复

`live_audio_capture_status` 显示捕获的状态、它的源和目标、播放是否已停止以及文件是否可用，但不显示路径或恢复令牌。它可以从新的宿主进程中使用。

如果状态不是 `cleaned`，请用 `confirmation: "emergency-stop-and-clean"` 和状态中显示的身份调用 `live_audio_capture_emergency_stop`。它会停止捕获，按上述方式检查并删除文件，然后才删除片段。如果路径、身份、格式或清理无法确认，它会报告还遗留了什么，并且不删除任何它无法担保的东西。只要还有遗留，就不要开始新的捕获。

## 核对分析结果

标准分析会用生成的音频（不使用第三方录音），与 FFmpeg 独立的 `ebur128` 滤镜进行核对：桥接的测试（`crates/ableton-mcp-server/tests/audio_standards.rs`）要求分析结果与 FFmpeg 对这些信号得出的结果一致，这些结果来自已记录的报告 [phase-8-audio-oracle.json](../evidence/phase-8-audio-oracle.json)（2026-07-27）。容差在 48 kHz 下为 0.1 LU 或 dB，对 44.1 kHz 的真峰值为 0.15 dBTP。已发布的标准仍然是定义；FFmpeg 只是交叉核对。

在真实 Live 上，Kumi 需要主动运行的验收（`accept_live`；见[测试](TESTING.md)）会播放、并轨并聆听。早先的捕获验证程序最近一次记录的运行 [phase-8-audio-live.json](../evidence/phase-8-audio-live.json) 是在 macOS 上的 Live 12.4.5b8 上进行的（2026-07-27，桥接 0.1.0），早于当前的桥接。没有记录任何 Windows 上的运行。

## 限制

- 真峰值只支持 44.1 和 48 kHz。
- 只支持常规的声道标签；不支持沉浸式或基于对象的布局。
- 捕获需要已保存的工程、WAV 录音以及可以恢复的路由。
- 没有 Max for Live 分接，没有插件电平表，不接受文件路径或 URL，不做时间拉伸，也不给出母带评级或合规结论。
- 删除文件不等于安全擦除。以你的用户身份运行的程序可以读取桥接的密钥，这超出了桥接的防护范围；在此范围内，文件检查防范的是失误和被调包的路径。
